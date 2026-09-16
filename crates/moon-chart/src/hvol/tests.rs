use super::*;

fn row(lo: f32, hi: f32, buy: f32, sell: f32) -> PriceProfileRow {
    PriceProfileRow {
        price_lo: lo,
        price_hi: hi,
        buy_quote: buy,
        sell_quote: sell,
    }
}

/// Moonbot's `Auto` table, row by row, at the boundaries the reference states.
///
/// Breakage: a `>` where `>=` belongs moves every boundary one zoom step, and the profile a reader
/// compares against the reference terminal covers a different window at the same zoom.
#[test]
fn auto_follows_the_reference_table_at_its_boundaries() {
    let m = |minutes: f64| auto_tf_s(minutes * 60_000.0);
    assert_eq!(m(1.9), 60);
    assert_eq!(m(2.0), 300);
    assert_eq!(m(3.9), 300);
    assert_eq!(m(4.0), 900);
    assert_eq!(m(9.9), 900);
    assert_eq!(m(10.0), 1_800);
    assert_eq!(m(19.9), 1_800);
    assert_eq!(m(20.0), 3_600);
    assert_eq!(m(29.9), 3_600);
    assert_eq!(m(30.0), 7_200);
    assert_eq!(m(59.9), 7_200);
    assert_eq!(m(60.0), 21_600);
    assert_eq!(m(119.9), 21_600);
    assert_eq!(m(120.0), 43_200);
    assert_eq!(m(6.0 * 60.0), 43_200, "Auto never reaches the day");
    assert_eq!(auto_tf_s(f64::NAN), 60);
}

/// `Auto` and `Max` pass through; anything else snaps onto the list.
#[test]
fn the_window_snaps_onto_the_list_with_auto_and_max_kept() {
    assert_eq!(snap_tf_s(0), 0);
    assert_eq!(snap_tf_s(HVOL_TF_MAX_S), HVOL_TF_MAX_S);
    assert_eq!(snap_tf_s(3_500), 3_600);
    assert_eq!(snap_tf_s(100_000), 86_400);
    assert_eq!(effective_window(HVOL_TF_MAX_S, 1.0), ProfileWindow::All);
    assert_eq!(
        effective_window(0, 25.0 * 60_000.0),
        ProfileWindow::Millis(3_600_000)
    );
    assert_eq!(effective_window(300, 0.0), ProfileWindow::Millis(300_000));
    assert_eq!(window_seconds(ProfileWindow::All), None);
    assert_eq!(window_seconds(ProfileWindow::Millis(90_000)), Some(90));
}

/// The window is a percentage of the price, floored at the tick and rounded to whole ticks.
///
/// Breakage: a window narrower than the tick holds nothing — the profile would draw every other
/// pixel empty and read as a comb.
#[test]
fn the_window_is_a_whole_number_of_ticks_never_below_one() {
    // ADA at 0.0247 with a 0.0001 tick: 0.12% asks for 0.00003, the tick wins — 0.40%.
    let w = window_width(0.12, 0.0247, Some(0.0001)).unwrap();
    assert!((w - 0.0001).abs() < 1e-12, "{w}");
    assert!((price_frame_pct_of(w, 0.0247) - 0.405).abs() < 0.01);
    // 1% of 100 with a 0.3 tick: 1.0 / 0.3 = 3.33 -> 3 ticks = 0.9.
    let w = window_width(1.0, 100.0, Some(0.3)).unwrap();
    assert!((w - 0.9).abs() < 1e-12, "{w}");
    // No tick known: the raw width stands.
    assert_eq!(window_width(0.5, 200.0, None), Some(1.0));
    assert_eq!(window_width(0.5, 200.0, Some(0.0)), Some(1.0));
    assert_eq!(window_width(0.5, 0.0, None), None);
    assert_eq!(window_width(0.5, f64::NAN, None), None);
}

/// The reference price follows the market only past the band, and never flaps inside it.
#[test]
fn the_reference_price_moves_only_past_the_band() {
    assert!(ref_price_moved(0.0, 10.0), "no reference yet");
    assert!(!ref_price_moved(10.0, 10.4));
    assert!(!ref_price_moved(10.0, 9.6));
    assert!(ref_price_moved(10.0, 10.6));
    assert!(ref_price_moved(10.0, 9.4));
    assert!(
        !ref_price_moved(10.0, f64::NAN),
        "an unusable price moves nothing"
    );
}

/// The visible maximum follows the kind: the larger side overlaid, the sum stacked — and only
/// rows touching the visible price range count.
#[test]
fn the_visible_maximum_follows_the_kind_and_the_price_range() {
    let rows = [
        row(1.0, 1.1, 10.0, 30.0),
        row(1.1, 1.2, 25.0, 20.0),
        row(5.0, 5.1, 100.0, 100.0),
    ];
    assert_eq!(visible_row_max(&rows, 0.9, 2.0, false), Some(30.0));
    assert_eq!(visible_row_max(&rows, 0.9, 2.0, true), Some(45.0));
    assert_eq!(visible_row_max(&rows, 0.9, 10.0, false), Some(100.0));
    assert_eq!(visible_row_max(&rows, 2.0, 4.0, false), None);
    assert_eq!(
        visible_row_max(&[row(1.0, 1.1, 0.0, 0.0)], 0.0, 9.0, true),
        None
    );
}

/// The cursor readout finds the row a price falls in, half-open at the top, and none between rows.
#[test]
fn the_row_under_a_price_is_found_by_its_half_open_band() {
    let rows = [
        row(1.0, 1.1, 1.0, 0.0),
        row(1.1, 1.2, 2.0, 0.0),
        row(5.0, 5.1, 3.0, 0.0),
    ];
    assert_eq!(row_at(&rows, 1.05).map(|r| r.buy_quote), Some(1.0));
    assert_eq!(
        row_at(&rows, 1.1).map(|r| r.buy_quote),
        Some(2.0),
        "the lower edge is inclusive"
    );
    assert_eq!(
        row_at(&rows, 1.2),
        None,
        "the upper edge is exclusive, and the gap has no row"
    );
    assert_eq!(row_at(&rows, 5.05).map(|r| r.buy_quote), Some(3.0));
    assert_eq!(row_at(&rows, f32::NAN), None);
    assert_eq!(row_at(&[], 1.0), None);
}

/// Every listed window has a label, `Max` has none, and the popup's steps agree with the caption's.
#[test]
fn every_listed_window_spells_itself() {
    let labels: Vec<String> = HVOL_TF_CHOICES_S
        .iter()
        .map(|s| tf_label(*s).expect("listed windows are nameable"))
        .collect();
    assert_eq!(
        labels,
        ["1m", "5m", "15m", "30m", "1h", "2h", "6h", "12h", "1d"]
    );
    assert_eq!(tf_label(HVOL_TF_MAX_S), None);
}

/// The normalizer clamps the horizontal-volume fields into their drawable ranges.
#[test]
fn normalize_clamps_the_hvol_fields() {
    let cfg = crate::normalize_chart_graphics(ChartGraphicsCfg {
        hvol_tf_s: 3_500,
        hvol_price_frame_pct: 99.0,
        hvol_width: f32::NAN,
        ..ChartGraphicsCfg::default()
    });
    assert_eq!(cfg.hvol_tf_s, 3_600);
    assert_eq!(cfg.hvol_price_frame_pct, PRICE_FRAME_PCT_MAX);
    assert_eq!(cfg.hvol_width, ChartGraphicsCfg::default().hvol_width);
    assert!(zone_spec(&cfg).is_none(), "off by default");
    let on = ChartGraphicsCfg {
        hvol_enabled: true,
        hvol_width: 0.9,
        ..ChartGraphicsCfg::default()
    };
    assert_eq!(
        zone_spec(&on),
        Some(HvolZoneSpec {
            width_frac: WIDTH_MAX
        })
    );
}

/// The zone's samples are the rolling sums over the window, one per pixel, bottom-up.
///
/// Breakage: this is the whole difference between the reference's continuous contour and a
/// stack of blocks — a sum that missed the bins beside a pixel's own would bring the blocks back;
/// one that never dropped them would smear the profile upward without limit.
#[test]
fn samples_are_rolling_sums_over_the_window_at_every_pixel() {
    // Bins 0.1 wide from 10.0; one trade at 10.05 (buy 8), one at 10.25 (sell 4).
    let bins = [row(10.0, 10.1, 8.0, 0.0), row(10.2, 10.3, 0.0, 4.0)];
    // A hundred pixels per unit of price, the zone's top at 10.5, 50 px tall: 10.0..10.5.
    let grid = SampleGrid {
        price_top: 10.5,
        px_per_price: 100.0,
        height_px: 50,
    };
    let mut out = Vec::new();
    // Window 0.1: a bin counts within ±0.05 of a pixel's centre.
    rolling_samples(&bins, 0.1, grid, &mut out);
    assert!(
        out.windows(2).all(|w| w[0].price_lo < w[1].price_lo),
        "sorted by price, bottom-up"
    );
    let at = |out: &[PriceProfileRow], p: f32| row_at(out, p).map(|r| (r.buy_quote, r.sell_quote));
    assert_eq!(at(&out, 10.05), Some((8.0, 0.0)), "the buy bin's own pixel");
    assert_eq!(
        at(&out, 10.09),
        Some((8.0, 0.0)),
        "still within half a window of its centre"
    );
    assert_eq!(
        at(&out, 10.15),
        None,
        "a pixel with no bin within half a window is empty"
    );
    assert_eq!(at(&out, 10.25), Some((0.0, 4.0)));
    // Window 0.5: both bins in one sum around the middle.
    rolling_samples(&bins, 0.5, grid, &mut out);
    assert_eq!(
        at(&out, 10.15),
        Some((8.0, 4.0)),
        "a wider window sums both bins"
    );
    assert_eq!(
        out.len(),
        50,
        "every pixel of the zone sees a bin within a quarter of price"
    );
    rolling_samples(&bins, 0.0, grid, &mut out);
    assert!(out.is_empty(), "no window, no samples");
    rolling_samples(&[], 0.1, grid, &mut out);
    assert!(out.is_empty());
}

/// Bins are the tick or a sixteenth of the window, whichever is wider.
#[test]
fn bins_are_the_tick_or_a_sixteenth_of_the_window() {
    assert!((bin_width(1.6, None) - 0.1).abs() < 1e-12);
    assert!(
        (bin_width(1.6, Some(0.5)) - 0.5).abs() < 1e-12,
        "a coarse tick wins"
    );
    assert!(
        (bin_width(1.6, Some(0.01)) - 0.1).abs() < 1e-12,
        "a fine tick is binned coarser"
    );
    assert!((bin_width(1.6, Some(0.0)) - 0.1).abs() < 1e-12, "no tick");
}
