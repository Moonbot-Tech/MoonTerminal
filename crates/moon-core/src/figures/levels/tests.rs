use super::*;

#[test]
fn zero_sits_at_the_end_of_the_move_and_one_at_its_start() {
    // The charting convention, and the half of it that is easy to invert by accident.
    assert_eq!(price_at(100.0, 80.0, 0.0), 80.0);
    assert_eq!(price_at(100.0, 80.0, 1.0), 100.0);
}

#[test]
fn a_retracement_lands_inside_the_move_whichever_way_it_went() {
    // Fall 100 → 80: 0.618 retraces 61.8% of the way back up from 80.
    let down = price_at(100.0, 80.0, 0.618);
    assert!((down - 92.36).abs() < 1e-9, "{down}");
    // Rise 80 → 100 is the same ratio measured the other way, and must not flip sign.
    let up = price_at(80.0, 100.0, 0.618);
    assert!((up - 87.64).abs() < 1e-9, "{up}");
    for p in [down, up] {
        assert!(
            (80.0..=100.0).contains(&p),
            "a retracement left the move: {p}"
        );
    }
}

#[test]
fn an_extension_continues_past_the_start_away_from_the_end() {
    // Fall 100 → 80, so extensions rise above 100.
    let ext = price_at(100.0, 80.0, 1.618);
    assert!(ext > 100.0, "{ext}");
    assert!((ext - 112.36).abs() < 1e-9, "{ext}");
    // Rise 80 → 100 extends the other way, below 80.
    let ext_up = price_at(80.0, 100.0, 1.618);
    assert!(ext_up < 80.0, "{ext_up}");
}

#[test]
fn the_default_scale_is_ordered_and_covers_the_move() {
    let ratios: Vec<f64> = FIB_LEVELS.iter().map(|l| l.ratio).collect();
    assert!(
        ratios.windows(2).all(|w| w[0] < w[1]),
        "levels must ascend — bands fill the gap between neighbours: {ratios:?}"
    );
    assert_eq!(ratios.first(), Some(&0.0));
    assert!(
        ratios.contains(&1.0),
        "the move's own start must be a level"
    );
    assert!(
        ratios.iter().any(|r| *r > 1.0),
        "a scale with no extension cannot show a target"
    );
}

#[test]
fn the_key_levels_are_the_ones_a_trader_watches() {
    let key: Vec<f64> = FIB_LEVELS
        .iter()
        .filter(|l| l.emphasis == Emphasis::Key)
        .map(|l| l.ratio)
        .collect();
    assert!(key.contains(&0.618), "the golden ratio is not emphasised");
    assert!(key.contains(&0.5));
    assert!(
        FIB_LEVELS
            .iter()
            .filter(|l| l.emphasis == Emphasis::Anchor)
            .count()
            == 2,
        "exactly the move's two ends are anchors"
    );
}

#[test]
fn the_levels_a_trader_watches_are_the_loudest() {
    // About OPACITY alone. Since every level gained its own hue, what the eye lands on first is
    // mostly colour — emphasis now decides the tie between two levels, not the whole ranking.
    assert!(
        Emphasis::Key.line_alpha() > Emphasis::Anchor.line_alpha(),
        "the golden group must not be the quietest thing on the scale"
    );
}

#[test]
fn neighbouring_levels_never_share_a_hue() {
    // Two levels of the same colour that touch cannot be told apart, and the band between them
    // takes the smaller ratio's hue — so a repeated colour also merges two bands into one wash.
    // Levels FAR apart may repeat (0 and 1 are both grey on purpose); adjacent ones may not.
    for pair in FIB_LEVELS.windows(2) {
        assert_ne!(
            pair[0].color, pair[1].color,
            "levels {} and {} share a colour",
            pair[0].ratio, pair[1].ratio
        );
    }
}

#[test]
fn the_moves_two_ends_are_the_same_colour_as_each_other() {
    // 0 and 1 bound the move itself rather than retrace it, and reading them as a pair is the
    // point of giving them one hue; a palette edit that splits them loses that.
    let anchors: Vec<[u8; 3]> = FIB_LEVELS
        .iter()
        .filter(|l| l.emphasis == Emphasis::Anchor)
        .map(|l| l.color)
        .collect();
    assert_eq!(anchors.len(), 2);
    assert_eq!(anchors[0], anchors[1], "the move's two ends read as a pair");
}

#[test]
fn no_level_is_drawn_in_a_colour_either_theme_swallows() {
    // The palette is fixed in both themes (see `Level::color`), so it has to survive both: a
    // near-black level disappears on the dark chart and a near-white one on the light chart, and
    // either reads as a missing line rather than a quiet one.
    for level in FIB_LEVELS {
        let [r, g, b] = level.color;
        assert!(
            r.max(g).max(b) >= 0x60,
            "level {} at {:?} is too dark for a dark chart",
            level.ratio,
            level.color
        );
        assert!(
            r.min(g).min(b) <= 0xC8,
            "level {} at {:?} is too pale for a light chart",
            level.ratio,
            level.color
        );
    }
}

#[test]
fn the_pickers_swatch_is_one_of_the_scales_own_hues() {
    // The popup shows ONE cell for a scale of eleven colours; if that cell drifts to a colour the
    // scale never draws, it stops being a preview and becomes a lie about what is about to appear.
    let swatch = super::scale_swatch();
    assert!(
        FIB_LEVELS.iter().any(|l| l.color == swatch),
        "the scale swatch {swatch:?} belongs to no level"
    );
}

#[test]
fn a_ratio_reads_as_a_trader_names_it() {
    // The switch labels and the on-chart readouts both go through this, so a change here renames
    // levels in two places at once.
    assert_eq!(super::fmt_ratio(0.618), "0.618");
    assert_eq!(super::fmt_ratio(0.5), "0.5", "trailing zeros must go");
    assert_eq!(super::fmt_ratio(1.0), "1", "a whole ratio keeps no point");
    assert_eq!(super::fmt_ratio(4.236), "4.236");
    assert_eq!(super::fmt_ratio(-0.0), "0", "-0 must not print as a level");
}

/// A ratio recovered by division is named by the level it cannot be told apart from — and only
/// then.
///
/// Every case here is a measurement, not a preference. The first is Moonbot's own single-precision
/// error on a wide move; the second is the same error on a narrow one, where it is visible in the
/// third decimal; the third and fourth are levels a user deliberately placed, which must keep their
/// own value however close a canonical name sits.
#[test]
fn a_recovered_ratio_is_named_only_inside_its_own_error() {
    // Wide move: the error is invisible and the level is already its own name.
    assert_eq!(snap_ratio(0.2360000014, 1853.0, 112.0), 0.236);
    // Narrow move on a big price: 0.236 comes back as 0.234375 and must still read as 0.236.
    assert_eq!(snap_ratio(0.234375, 60000.0, 1.0), 0.236);
    // A quarter retracement is not a mis-measured 0.236, however wide the bar would like to be.
    assert_eq!(snap_ratio(0.25, 60000.0, 1.0), 0.25);
    // The nearest name wins, not the first in the table: 1.272 sits next to 1.236.
    assert_eq!(snap_ratio(1.272, 60000.0, 0.5), 1.272);
}

/// Degenerate inputs leave the ratio exactly as measured rather than snapping it to anything.
#[test]
fn snapping_refuses_a_span_it_cannot_reason_about() {
    for (ratio, price, span) in [
        (0.2341, 60000.0, 0.0),
        (0.2341, 60000.0, f64::NAN),
        (0.2341, f64::INFINITY, 1.0),
        (f64::NAN, 60000.0, 1.0),
    ] {
        let got = snap_ratio(ratio, price, span);
        assert!(got == ratio || (got.is_nan() && ratio.is_nan()), "{ratio} {price} {span}");
    }
}

/// A ratio the third decimal cannot see prints as zero, sign included.
///
/// Reachable only since a ratio can be RECOVERED by division: a fixed scale never produces
/// -0.0000022, and "-0" on a chart reads as a defect.
#[test]
fn a_ratio_below_the_printed_precision_loses_its_sign() {
    assert_eq!(fmt_ratio(-0.0000022), "0");
    assert_eq!(fmt_ratio(0.0000022), "0");
    assert_eq!(fmt_ratio(-0.0006), "-0.001");
}
