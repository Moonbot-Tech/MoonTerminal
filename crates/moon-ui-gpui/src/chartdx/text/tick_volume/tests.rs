//! Independent font-advance fixtures exercise narrow layout without opening a GPU window.

use super::{fit_tick_readout, tick_amount, tick_lines, tick_readout_origin};
use moon_chart::tick_volume::TickVolumeRange;

/// Measured from bundled GeistMono-Regular.ttf with fontTools: head.unitsPerEm = 1000,
/// hmtx advance = 600 for digits, punctuation, Latin and Cyrillic characters used here.
/// This fixture is independent of rough_label_width and the production layout arithmetic.
fn geist_default(text: &str) -> [f32; 2] {
    [text.chars().count() as f32 * 6.9, 15.5]
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
        assert_eq!(readout.height, 108.5);
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
        let readout = fit_tick_readout(extremes, "USDT", locale, [200.0, 180.0], geist_default)
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
    let wide = fit_tick_readout(ranges, "BTC", "en", [200.0, 180.0], geist_default)
        .expect("single side fits");
    assert_eq!(wide.lines[1].0, "SELL (42): 12 345 678 BTC");
    // This shorter SELL-only line fits at 200px; 160px forces stacking while its heading fits.
    let readout = fit_tick_readout(ranges, "BTC", "en", [160.0, 180.0], geist_default)
        .expect("single side stacks without dropping its count or quote");
    let lines: Vec<_> = readout
        .lines
        .iter()
        .map(|(text, _)| text.as_str())
        .collect();
    assert_eq!(lines, ["Nearby ticks (BTC)", "SELL (42)", "12 345 678"]);
}

/// Device scaling and pane offsets must not let the last line or rightmost digit cross its pane.
#[test]
fn narrow_backdrops_stay_inside_offset_panes_at_multiple_dpis() {
    for sf in [1.0, 1.25, 1.5, 2.0] {
        let bounds = [83.0 * sf, 41.0 * sf, 200.0 * sf, 180.0 * sf];
        let readout = fit_tick_readout(
            overlapping_ranges(),
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
    let wide = tick_lines(ranges, "USDT", "en", false, false);
    let measured = |text: &str| {
        if text == wide[1] || text == wide[2] {
            [500.0, 18.0]
        } else {
            [160.0, 18.0]
        }
    };
    let fit = fit_tick_readout(ranges, "USDT", "en", [174.0, 140.0], measured).expect("exact fit");
    assert_eq!(fit.lines.len(), 7);
    assert!(fit_tick_readout(ranges, "USDT", "en", [173.9, 140.0], measured).is_none());
    assert!(fit_tick_readout(ranges, "USDT", "en", [174.0, 139.9], measured).is_none());
    assert!(fit_tick_readout([None, None], "USDT", "en", [200.0, 180.0], geist_default).is_none());
    assert!(fit_tick_readout(ranges, "", "en", [200.0, 180.0], geist_default).is_none());
}
