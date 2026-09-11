use super::*;
use moon_core::market::candles::{CandleSeries, candle_intersects_window};

/// Builds one volume-only candle for a deterministic visibility scenario.
fn candle(t_open_ms: f64, volume: f32) -> ChartCandle {
    ChartCandle {
        t_open_ms,
        volume,
        quote_volume: volume,
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
            quote_volume: 100.0,
        },
        VolumeSample {
            t_open_ms: 5.0,
            tf_ms: 10.0,
            quote_volume: 10.0,
        },
        VolumeSample {
            t_open_ms: 20.0,
            tf_ms: 10.0,
            quote_volume: 30.0,
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
        quote_volume: 0.0,
    }];
    assert_eq!(visible_volume_stats(&zeros, 0.0, 10.0), None);
}

/// `volume_bars.rs:collect_samples` reading base `ChartCandle::volume` instead of quote turnover
/// restores the misleading million-unit band while the market's actual turnover is modest.
#[test]
fn collected_samples_use_quote_turnover_and_clamp_only_that_value() {
    let candles = [
        ChartCandle {
            t_open_ms: 0.0,
            volume: 1_280_000.0,
            quote_volume: 128.0,
            ..ChartCandle::default()
        },
        ChartCandle {
            t_open_ms: 10.0,
            volume: 1.0,
            quote_volume: -2.0,
            ..ChartCandle::default()
        },
    ];
    let mut samples = Vec::new();
    collect_samples(&candles, &[10.0, 10.0], 10.0, &mut samples);
    assert_eq!(
        samples[0].quote_volume, 128.0,
        "the independently supplied quote turnover is the band input"
    );
    assert_eq!(
        samples[1].quote_volume, 0.0,
        "negative quote turnover cannot build a reciprocal scale"
    );
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

/// One retained bucket, for the cursor lookup.
fn sample(t_open_ms: f64, tf_ms: f64, quote_volume: f32) -> VolumeSample {
    VolumeSample {
        t_open_ms,
        tf_ms,
        quote_volume,
    }
}

/// `sample_at` reads each bucket against its OWN width, half-open, and prefers the narrowest.
///
/// Breakage this pins: judging membership against one series-wide width would place a cursor over
/// the coarse history tail in the wrong bucket (or in none), and a closed upper bound would make
/// two adjacent buckets both claim the instant on their shared edge.
#[test]
fn cursor_lookup_uses_each_buckets_own_half_open_width() {
    let samples = [
        sample(1_000.0, 60_000.0, 10.0),
        sample(61_000.0, 60_000.0, 20.0),
        // Coarse history filler, reaching past both fine buckets as the composed tail does.
        sample(1_000.0, 240_000.0, 999.0),
        // A wide tail bucket after a gap.
        sample(300_000.0, 3_600_000.0, 30.0),
    ];

    // Where a fine bucket and the coarse filler both cover the instant, the narrowest wins, so
    // the readout names the bar the chart actually drew.
    assert_eq!(sample_at(&samples, 1_000.0), Some(samples[0]));
    assert_eq!(sample_at(&samples, 60_999.0), Some(samples[0]));
    // Half-open: the shared edge belongs to the later bucket alone.
    assert_eq!(sample_at(&samples, 61_000.0), Some(samples[1]));
    // Past both fine buckets, only the coarse filler still covers the instant.
    assert_eq!(sample_at(&samples, 121_000.0), Some(samples[2]));
    assert_eq!(sample_at(&samples, 240_999.0), Some(samples[2]));
    // The gap between the filler's end and the tail bucket answers nothing.
    assert_eq!(sample_at(&samples, 241_000.0), None);
    assert_eq!(sample_at(&samples, 300_000.0), Some(samples[3]));
    assert_eq!(sample_at(&samples, 3_900_000.0), None);
    // Before the first bucket, and on unusable inputs.
    assert_eq!(sample_at(&samples, 999.0), None);
    assert_eq!(sample_at(&samples, f64::NAN), None);
    assert_eq!(sample_at(&[], 1_000.0), None);
    assert_eq!(
        sample_at(
            &[sample(1_000.0, 0.0, 5.0), sample(f64::NAN, 60_000.0, 5.0)],
            1_000.0
        ),
        None
    );
}

/// `bucket_label` spells a bucket width the way the chart's own timeframe controls do.
///
/// Breakage this pins: dropping the period from the readout, or naming a 60-minute bucket `60m`,
/// makes a one-minute total and a one-hour total read alike in the same pane.
#[test]
fn bucket_label_matches_the_charts_own_timeframe_tokens() {
    assert_eq!(bucket_label(60_000.0).as_deref(), Some("1m"));
    assert_eq!(bucket_label(300_000.0).as_deref(), Some("5m"));
    assert_eq!(bucket_label(3_600_000.0).as_deref(), Some("1h"));
    assert_eq!(bucket_label(14_400_000.0).as_deref(), Some("4h"));
    assert_eq!(bucket_label(86_400_000.0).as_deref(), Some("1d"));
    assert_eq!(bucket_label(30_000.0).as_deref(), Some("30s"));
    assert_eq!(bucket_label(90_000.0).as_deref(), Some("90s"));
    assert_eq!(bucket_label(250.0).as_deref(), Some("250ms"));
    for bad in [0.0, 0.5, -60_000.0, f64::NAN, f64::INFINITY] {
        assert_eq!(bucket_label(bad), None, "{bad} cannot be named");
    }
}
