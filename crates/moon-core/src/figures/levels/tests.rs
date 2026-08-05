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
    assert!(
        Emphasis::Key.line_alpha() > Emphasis::Anchor.line_alpha(),
        "the golden group must not be the quietest thing on the scale"
    );
}
