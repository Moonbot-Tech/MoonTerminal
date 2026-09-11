//! Independent font-advance fixtures exercise narrow layout without opening a GPU window.

use super::{CandleVolume, fit_tick_readout, tick_amount, tick_lines, tick_readout_origin};
use moon_chart::tick_volume::TickVolumeRange;

/// Native band hit testing must reject the candle plot, axes and hidden bands at every scale.
#[test]
fn volume_readout_requires_hovering_a_visible_band() {
    use crate::chartdx::types::VolumeStyleGpu;
    let candle = VolumeStyleGpu {
        up: [0.0, 1.0, 0.0, 0.5],
        down: [1.0, 0.0, 0.0, 0.5],
        m: [1.0, 0.10, 1.0, 0.0],
        ..VolumeStyleGpu::default()
    };
    for scale in [1.0, 1.5, 2.0] {
        let bounds = [100.0 * scale, 50.0 * scale, 600.0 * scale, 800.0 * scale];
        let base = 850.0 * scale - 1.0;
        let pick = |x, y, alpha, style| super::hovered_volume_bands(bounds, [x, y], alpha, style);
        assert_eq!(
            pick(300.0 * scale, 300.0 * scale, 0.5, candle),
            [false, false]
        );
        assert_eq!(pick(300.0 * scale, base - 20.0, 0.5, candle), [true, true]);
        assert_eq!(
            pick(300.0 * scale, base - 73.0, 0.5, candle),
            [false, true],
            "native tick cap is 72 device pixels, not logical pixels"
        );
        assert_eq!(
            pick(300.0 * scale, base - 81.0 * scale, 0.5, candle),
            [false, false]
        );
        assert_eq!(pick(99.0 * scale, base, 0.5, candle), [false, false]);
        assert_eq!(pick(701.0 * scale, base, 0.5, candle), [false, false]);
        assert_eq!(pick(300.0 * scale, base + 2.0, 0.5, candle), [false, false]);
        assert_eq!(pick(300.0 * scale, base - 20.0, 0.0, candle), [false, true]);
        assert_eq!(
            pick(300.0 * scale, base - 20.0, 0.5, VolumeStyleGpu::default()),
            [true, false]
        );
        let hidden = VolumeStyleGpu {
            up: [0.0; 4],
            down: [0.0; 4],
            ..candle
        };
        assert_eq!(
            pick(300.0 * scale, base - 20.0, 0.0, hidden),
            [false, false]
        );
    }
    let small = [0.0, 0.0, 200.0, 100.0];
    assert_eq!(
        super::hovered_volume_bands(small, [50.0, 81.0], 1.0, candle),
        [true, false]
    );
    assert_eq!(
        super::hovered_volume_bands(small, [50.0, 80.0], 1.0, candle),
        [false, false]
    );
    assert_eq!(
        super::hovered_volume_bands(small, [50.0, f32::NAN], 1.0, candle),
        [false, false]
    );
}

/// Measured from bundled GeistMono-Regular.ttf with fontTools: head.unitsPerEm = 1000,
/// hmtx advance = 600 for digits, punctuation, Latin and Cyrillic characters used here.
/// At the requested 13px default, the independent 0.6em advance is 7.8px and the
/// four-pixel leading gives a 17px line. Do not derive these from production constants.
fn geist_default(text: &str) -> [f32; 2] {
    [text.chars().count() as f32 * 7.8, 17.0]
}

/// Nearby BUY and SELL columns with distinct adjacent amounts that SI rounding would collapse.
fn overlapping_ranges() -> [Option<TickVolumeRange>; 2] {
    [
        Some(TickVolumeRange {
            count: 123456,
            min: 12345678.0,
            max: 12345679.0,
        }),
        Some(TickVolumeRange {
            count: 234567,
            min: 9876542.0,
            max: 9876543.0,
        }),
    ]
}

/// A wide pane must retain the original decimal/count/range/unit presentation.
#[test]
fn wide_readout_keeps_original_lines() {
    let readout = fit_tick_readout(
        overlapping_ranges(),
        None,
        "USDT",
        "en",
        [800.0, 180.0],
        geist_default,
    )
    .expect("wide readout");
    let lines: Vec<_> = readout
        .lines
        .iter()
        .map(|(text, _)| text.as_str())
        .collect();
    assert_eq!(
        lines,
        [
            "Nearby ticks",
            "BUY (123456): 12 345 678 - 12 345 679 USDT",
            "SELL (234567): 9 876 542 - 9 876 543 USDT",
        ]
    );
}

/// Both overlapping sides stay readable in useful narrow docks in every supported language.
#[test]
fn narrow_dock_stacks_both_counts_and_exact_ranges() {
    for locale in ["en", "ru", "es"] {
        let readout = fit_tick_readout(
            overlapping_ranges(),
            None,
            "USDT",
            locale,
            [200.0, 180.0],
            geist_default,
        )
        .expect("200x180 narrow plot must retain both ranges");
        let lines: Vec<_> = readout
            .lines
            .iter()
            .map(|(text, _)| text.as_str())
            .collect();
        assert_eq!(lines.len(), 7);
        assert!(lines[0].contains("USDT"));
        assert_eq!(lines[1], "BUY (123456)");
        assert!(lines[2].ends_with("12 345 678"));
        assert!(lines[3].ends_with("12 345 679"));
        assert_eq!(lines[4], "SELL (234567)");
        assert!(lines[5].ends_with("9 876 542"));
        assert!(lines[6].ends_with("9 876 543"));
        // Independent safe rectangle: 5px backing plus 2px inset at both horizontal edges.
        assert!(readout.width <= 186.0);
        assert_eq!(readout.height, 119.0);
    }
}

/// Very large/small f32 amounts remain exact and never collapse to the same rounded suffix.
#[test]
fn scientific_fallback_round_trips_extremes_and_adjacent_values() {
    for value in [
        f32::MAX,
        f32::MIN_POSITIVE,
        f32::from_bits(1),
        12345678.0,
        12345679.0,
    ] {
        let text = tick_amount(value, true);
        assert_eq!(
            text.replace(' ', "")
                .parse::<f32>()
                .expect("numeric amount")
                .to_bits(),
            value.to_bits()
        );
    }
    let extremes = [Some(TickVolumeRange {
        count: 999999,
        min: f32::from_bits(1),
        max: f32::MAX,
    }); 2];
    for locale in ["en", "ru", "es"] {
        let readout = fit_tick_readout(
            extremes,
            None,
            "USDT",
            locale,
            [200.0, 180.0],
            geist_default,
        )
        .expect("extremes fit without rounding or clipping");
        assert_eq!(readout.lines.len(), 7);
        assert!(readout.lines[2].0.ends_with("1e-45"));
        assert!(readout.lines[3].0.ends_with("3.4028235e38"));
        assert!(readout.width <= 186.0);
    }
}

/// Equal-size prints still retain their count, and a missing BUY never relabels SELL.
#[test]
fn equal_sell_prints_keep_count_value_and_quote() {
    let ranges = [
        None,
        Some(TickVolumeRange {
            count: 42,
            min: 12345678.0,
            max: 12345678.0,
        }),
    ];
    let wide = fit_tick_readout(ranges, None, "BTC", "en", [240.0, 180.0], geist_default)
        .expect("single side fits");
    assert_eq!(wide.lines[1].0, "SELL (42): 12 345 678 BTC");
    // At 13px the inline line needs more than 200px, but both narrow sizes retain all facts.
    for width in [200.0, 160.0] {
        let readout = fit_tick_readout(ranges, None, "BTC", "en", [width, 180.0], geist_default)
            .expect("single side stacks without dropping its count or quote");
        let lines: Vec<_> = readout
            .lines
            .iter()
            .map(|(text, _)| text.as_str())
            .collect();
        assert_eq!(lines, ["Nearby ticks (BTC)", "SELL (42)", "12 345 678"]);
    }
}

/// Device scaling and pane offsets must not let the last line or rightmost digit cross its pane.
#[test]
fn narrow_backdrops_stay_inside_offset_panes_at_multiple_dpis() {
    for sf in [1.0, 1.25, 1.5, 2.0] {
        let bounds = [83.0 * sf, 41.0 * sf, 200.0 * sf, 180.0 * sf];
        let readout = fit_tick_readout(
            overlapping_ranges(),
            None,
            "USDT",
            "en",
            [bounds[2] / sf, bounds[3] / sf],
            geist_default,
        )
        .expect("logical size survives DPI");
        for cursor in [83.0 * sf, 283.0 * sf] {
            let [x, mut y] = tick_readout_origin(bounds, sf, cursor, &readout);
            for (_, [width, height]) in &readout.lines {
                assert!(x - 5.0 >= 83.0);
                assert!(x + width + 5.0 <= 283.0);
                assert!(y - 2.5 >= 41.0);
                assert!(y + height + 2.5 <= 221.0);
                y += height;
            }
        }
    }
}

/// The real measurement callback controls fit; font changes cannot sneak past a character guess.
#[test]
fn fit_honors_measured_width_height_and_exact_boundary() {
    let ranges = overlapping_ranges();
    let wide = tick_lines(ranges, None, "USDT", "en", false, false);
    let measured = |text: &str| {
        if text == wide[1] || text == wide[2] {
            [500.0, 18.0]
        } else {
            [160.0, 18.0]
        }
    };
    let fit =
        fit_tick_readout(ranges, None, "USDT", "en", [174.0, 140.0], measured).expect("exact fit");
    assert_eq!(fit.lines.len(), 7);
    assert!(fit_tick_readout(ranges, None, "USDT", "en", [173.9, 140.0], measured).is_none());
    assert!(fit_tick_readout(ranges, None, "USDT", "en", [174.0, 139.9], measured).is_none());
    assert!(
        fit_tick_readout(
            [None, None],
            None,
            "USDT",
            "en",
            [200.0, 180.0],
            geist_default
        )
        .is_none()
    );
    assert!(fit_tick_readout(ranges, None, "", "en", [200.0, 180.0], geist_default).is_none());
}

/// One hovered bucket: a 1-minute candle with a real turnover figure.
fn minute_candle() -> Option<CandleVolume> {
    Some(CandleVolume {
        quote: 12_345_678.0,
        tf_ms: 60_000.0,
    })
}

/// The candle's aggregate is labelled as a candle, never folded into the tick rows.
///
/// Breakage this pins: appending the bucket total as another `BUY`/`SELL` row, or summing it into
/// one, would make a period's turnover read as a single print — the one thing this block must never
/// say. The period token beside it is what separates a minute's total from a day's.
#[test]
fn candle_volume_is_a_separate_labelled_aggregate() {
    let readout = fit_tick_readout(
        overlapping_ranges(),
        minute_candle(),
        "USDT",
        "en",
        [800.0, 180.0],
        geist_default,
    )
    .expect("wide readout");
    let lines: Vec<_> = readout
        .lines
        .iter()
        .map(|(text, _)| text.as_str())
        .collect();
    assert_eq!(
        lines,
        [
            "Nearby ticks",
            "BUY (123456): 12 345 678 - 12 345 679 USDT",
            "SELL (234567): 9 876 542 - 9 876 543 USDT",
            "Candle 1m: 12 345 678 USDT",
        ]
    );
    // No side word anywhere on the candle line, in any language.
    for locale in ["en", "ru", "es"] {
        let line = &tick_lines([None, None], minute_candle(), "USDT", locale, false, false)[0];
        assert!(!line.contains("BUY") && !line.contains("SELL"), "{line}");
        assert!(line.contains("1m") && line.contains("USDT"), "{line}");
    }
}

/// A candle alone still produces a readout, which is what makes the candle plot answer at all.
///
/// Breakage this pins: requiring a tick range before anything is drawn — the former behaviour —
/// leaves a cursor over the candles with no volume readout, and the tick heading must not appear
/// over a block that holds no ticks.
#[test]
fn candle_alone_draws_without_the_tick_heading() {
    for locale in ["en", "ru", "es"] {
        let readout = fit_tick_readout(
            [None, None],
            minute_candle(),
            "USDT",
            locale,
            [240.0, 180.0],
            geist_default,
        )
        .expect("a hovered candle is enough");
        assert_eq!(readout.lines.len(), 1);
        let line = readout.lines[0].0.as_str();
        // Unit stays on the line itself: there is no heading here to carry it.
        assert!(line.ends_with("USDT"), "{line}");
        assert!(
            !line.contains(&t_nearby(locale)),
            "a tick heading over a tick-less block: {line}"
        );
    }
    assert!(
        fit_tick_readout(
            [None, None],
            None,
            "USDT",
            "en",
            [200.0, 180.0],
            geist_default
        )
        .is_none()
    );
}

/// The tick heading as this locale spells it, for the "no heading without ticks" assertion.
fn t_nearby(locale: &str) -> String {
    rust_i18n::t!("tick_volume.nearby", locale = locale).to_string()
}

/// Stacked, the candle borrows the tick heading's unit only while that heading exists.
///
/// Breakage this pins: printing the bare amount with no unit anywhere — the shape a candle-only
/// stacked readout would take if it inherited a heading it does not have.
#[test]
fn stacked_candle_keeps_its_unit_reachable() {
    // With ticks: the stacked heading carries the unit, so the candle prints head + amount.
    let with_ticks = tick_lines(
        overlapping_ranges(),
        minute_candle(),
        "USDT",
        "en",
        true,
        false,
    );
    assert!(with_ticks[0].contains("USDT"));
    assert_eq!(with_ticks[with_ticks.len() - 2], "Candle 1m");
    assert_eq!(with_ticks[with_ticks.len() - 1], "12 345 678");
    // Without ticks: no heading above, so the candle's own heading carries the unit.
    let alone = tick_lines([None, None], minute_candle(), "USDT", "en", true, false);
    assert_eq!(alone, ["Candle 1m (USDT)", "12 345 678"]);
}

/// A candle-only readout must be able to NARROW, or a useful dock loses the block entirely.
///
/// Breakage this pins: handing `candle_lines` a stacked flag that is false whenever there are no
/// ticks. Every attempt then produces the same one-line inline form, its width never drops, and a
/// pane too narrow for that line shows no volume at all — while the same pane fits the stacked
/// heading and amount comfortably.
#[test]
fn a_candle_only_readout_stacks_into_a_narrow_pane() {
    for locale in ["en", "ru", "es"] {
        // Too narrow for the inline `Candle 1m: 12 345 678 USDT` at this fixture's advance.
        let readout = fit_tick_readout(
            [None, None],
            minute_candle(),
            "USDT",
            locale,
            [140.0, 180.0],
            geist_default,
        )
        .expect("a candle-only readout must narrow rather than vanish");
        assert_eq!(readout.lines.len(), 2, "heading plus amount");
        assert!(readout.lines[0].0.contains("USDT"), "unit stays reachable");
        assert!(readout.lines[0].0.contains("1m"), "period stays named");
        assert_eq!(readout.lines[1].0, "12 345 678");
        assert!(readout.width + 14.0 <= 140.0);
    }
}

/// Both tick ranges and the candle total remain complete at the enlarged default size.
#[test]
fn combined_default_readout_fits_narrow_panes_at_multiple_dpis() {
    for locale in ["en", "ru", "es"] {
        for sf in [1.0, 1.25, 1.5, 2.0] {
            let bounds = [83.0 * sf, 41.0 * sf, 200.0 * sf, 180.0 * sf];
            let readout = fit_tick_readout(
                overlapping_ranges(),
                minute_candle(),
                "USDT",
                locale,
                [bounds[2] / sf, bounds[3] / sf],
                geist_default,
            )
            .expect("both tick ranges and candle fit at 13px");
            assert_eq!(readout.lines.len(), 9);
            assert_eq!(readout.height, 153.0);
            assert_eq!(readout.lines[1].0, "BUY (123456)");
            assert_eq!(readout.lines[4].0, "SELL (234567)");
            assert!(readout.lines[7].0.contains("1m"));
            assert_eq!(readout.lines[8].0, "12 345 678");
            let [x, y] = tick_readout_origin(bounds, sf, 283.0 * sf, &readout);
            assert!(x - 5.0 >= 83.0 && x + readout.width + 5.0 <= 283.0);
            assert!(y - 2.5 >= 41.0 && y + readout.height + 2.5 <= 221.0);
        }
    }
}

/// A four-pixel label increase must affect fitting, including the final candle line.
#[test]
fn increased_font_respects_the_combined_readout_height_boundary() {
    // The same bundled 0.6em glyph advance at 17px, with four pixels of leading.
    let measure = |text: &str| [text.chars().count() as f32 * 10.2, 21.0];
    for locale in ["en", "ru", "es"] {
        let fit = fit_tick_readout(
            overlapping_ranges(),
            minute_candle(),
            "USDT",
            locale,
            [260.0, 203.0],
            measure,
        )
        .expect("nine 21px lines and fourteen pixels of clearance fit exactly");
        assert_eq!(fit.lines.len(), 9);
        assert_eq!(fit.lines[8].0, "12 345 678");
        assert!(
            fit_tick_readout(
                overlapping_ranges(),
                minute_candle(),
                "USDT",
                locale,
                [260.0, 202.0],
                measure,
            )
            .is_none(),
            "an undersized pane must not clip the candle total"
        );
    }
}

/// A turnover figure that is not a real positive amount contributes no line at all.
///
/// Breakage this pins: dropping the finite check and printing `inf USDT` where money is expected.
/// `collect_samples` floors at zero, which absorbs a NaN but leaves an infinity intact.
#[test]
fn a_non_finite_or_empty_bucket_contributes_no_candle_line() {
    for bad in [f32::INFINITY, f32::NEG_INFINITY, f32::NAN, 0.0, -1.0] {
        assert_eq!(CandleVolume::new(bad, 60_000.0), None, "{bad} is not money");
    }
    assert_eq!(
        CandleVolume::new(12_345_678.0, 60_000.0),
        minute_candle(),
        "a real positive turnover is kept unchanged"
    );
}

/// A bucket whose width cannot be named prints the amount without inventing a period.
#[test]
fn an_unnameable_bucket_width_drops_the_period_not_the_figure() {
    let candle = Some(CandleVolume {
        quote: 42.0,
        tf_ms: 0.5,
    });
    assert_eq!(
        tick_lines([None, None], candle, "USDT", "en", false, false),
        ["Candle: 42 USDT"]
    );
    assert_eq!(
        tick_lines(overlapping_ranges(), candle, "USDT", "en", true, false)
            .last()
            .map(String::as_str),
        Some("42")
    );
    // Candle-only and stacked: the unit rides the untimed heading rather than being lost.
    assert_eq!(
        tick_lines([None, None], candle, "USDT", "en", true, false),
        ["Candle (USDT)", "42"]
    );
}
