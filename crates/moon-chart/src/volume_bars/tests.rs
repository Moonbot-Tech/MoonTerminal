use super::*;

/// Builds one volume-only candle for a deterministic visibility scenario.
fn candle(t_open_ms: f64, volume: f32) -> ChartCandle {
    ChartCandle {
        t_open_ms,
        volume,
        quote_volume: volume,
        ..ChartCandle::default()
    }
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
    // Read at the coarse bucket's own interval, so the figure is the bucket's turnover itself.
    assert_eq!(
        visible_interval_max(&samples, 120.0, 130.0, 100.0, f64::INFINITY),
        Some(8.0),
        "the coarse bucket intersects and must set the scale"
    );
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

/// The bracket's rule is one function for the text pass and, through the signed inset, for the
/// shaders: the left stem sits a few pixels in, the right one a fixed distance short of the edge,
/// and neither leaves the plot when it is narrower than that distance.
#[test]
fn scale_bracket_offset_follows_the_reference_and_stays_inside_the_plot() {
    assert_eq!(
        scale_bracket_offset(1000.0, false),
        VOLUME_SCALE_LEFT_INSET_PX
    );
    assert_eq!(
        scale_bracket_offset(1000.0, true),
        1000.0 - VOLUME_SCALE_RIGHT_INSET_PX
    );
    // Narrower than the right inset: the stem lands on the left edge, not past it.
    assert_eq!(scale_bracket_offset(100.0, true), 0.0);
    // A degenerate plot never yields a negative offset.
    assert_eq!(scale_bracket_offset(-5.0, false), 0.0);
    // The shader-side form applies the same clamp once the plot width is known.
    for (w, right) in [(1000.0, false), (1000.0, true), (100.0, true)] {
        let signed = scale_bracket_signed_inset(right);
        let shader_form = if signed >= 0.0 { signed } else { w + signed }.clamp(0.0, w);
        assert_eq!(shader_form, scale_bracket_offset(w, right));
    }
}

/// `volume_bars.rs:clamp_volume_style` / `volume_band_on` — every pair an older build could
/// store that drew ANY volume folds onto hills with the split on; only OFF with the split off
/// stays off. A rule that read the style alone would drop the split-only users, and one that
/// read the split alone would drop everyone on plain hills or bars.
#[test]
fn every_stored_pair_that_drew_volumes_reads_as_hills_with_the_split() {
    use moon_core::market::candles::{
        VOLUME_STYLE_HILLS, VOLUME_STYLE_LEGACY_BARS, VOLUME_STYLE_LEGACY_SIDES, VOLUME_STYLE_OFF,
    };
    for (style, sides) in [
        (VOLUME_STYLE_HILLS, false),
        (VOLUME_STYLE_HILLS, true),
        (VOLUME_STYLE_LEGACY_BARS, false),
        (VOLUME_STYLE_LEGACY_BARS, true),
        (VOLUME_STYLE_LEGACY_SIDES, false),
        (VOLUME_STYLE_OFF, true),
        (u8::MAX, false),
    ] {
        assert!(
            volume_band_on(style, sides),
            "({style}, {sides}) drew volumes"
        );
        assert_eq!(clamp_volume_style(style, sides), VOLUME_STYLE_HILLS);
    }
    assert!(!volume_band_on(VOLUME_STYLE_OFF, false));
    assert_eq!(
        clamp_volume_style(VOLUME_STYLE_OFF, false),
        VOLUME_STYLE_OFF
    );
}

/// `volume_bars.rs:sample_at_sorted` flipping its equal-width tie rule (`<=` to `<`) lets the
/// later of two overlapping same-width buckets win, and `visible_interval_max_sorted` losing a
/// bound drops a visible bucket, so the volume readout names the wrong bucket or the wrong peak.
/// Both are compared with the order-free linear references over random sorted mixed-width rows.
#[test]
fn sorted_lookups_equal_the_linear_references() {
    let mut state = 0x0F0F_1234_u64;
    let mut next = move |n: u64| {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state % n
    };
    let widths = [60_000.0, 300_000.0, 900_000.0];
    let mut samples = Vec::new();
    let mut t = 0.0;
    for _ in 0..3_000 {
        // Opens advance by less than a bucket, so same-width buckets overlap and tie.
        t += (next(4) * 30_000) as f64;
        samples.push(VolumeSample {
            t_open_ms: t,
            tf_ms: widths[next(3) as usize],
            quote_volume: 1.0 + next(10_000) as f32,
        });
    }
    let max_tf = 900_000.0;
    let (mut ties, mut hits) = (0, 0);
    for q in 0..5_000 {
        let at = (next(t as u64 + 1_000_000)) as f64 + if q % 2 == 0 { 0.0 } else { 0.5 };
        take_sample_at_visited();
        let got = sample_at_sorted(&samples, max_tf, at);
        let visited = take_sample_at_visited();
        let want = sample_at(&samples, at);
        assert_eq!(got, want, "t {at}");
        if let Some(w) = want {
            hits += 1;
            ties += usize::from(
                samples
                    .iter()
                    .filter(|s| {
                        s.tf_ms == w.tf_ms && at >= s.t_open_ms && at < s.t_open_ms + s.tf_ms
                    })
                    .count()
                    > 1,
            );
        }
        assert!(
            visited <= 2 + (max_tf / 30_000.0) as u64 * 2,
            "visited {visited}"
        );
        if q % 2_000 == 0 {
            println!("[sample_at] before={} after={visited}", samples.len());
        }

        let from = at;
        let to = from + (next(40) * 60_000) as f64;
        let boundary = if q % 3 == 0 {
            f64::INFINITY
        } else {
            from + (next(50) * 60_000) as f64
        };
        take_interval_max_visited();
        let got = visible_interval_max_sorted(&samples, max_tf, from, to, 60_000.0, boundary);
        let visited = take_interval_max_visited();
        assert_eq!(
            got,
            visible_interval_max(&samples, from, to, 60_000.0, boundary),
            "[{from}, {to}]"
        );
        if q % 2_000 == 0 {
            println!("[interval_max] before={} after={visited}", samples.len());
        }
    }
    assert!(hits > 0 && ties > 0, "{hits} {ties}");
}
