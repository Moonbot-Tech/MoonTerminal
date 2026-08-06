use super::*;
use crate::figures::tools::tests::TestProj;

/// The downward live sample: `a` is the higher price, so its first slot lands on `b`.
fn down_sample() -> MbFib {
    MbFib {
        a: 2026.76,
        b: 1997.2145,
        time_ms: 1_754_400_000_000.0,
        levels: [
            1997.2145, 2019.7873, 2015.4736, 2011.9872, 2008.5009, 2003.5372, 1990.2417,
        ],
    }
}

/// The sample decoded from the live blob (core 17 «QQ», ETHUSD_PERP, 2026-08-05 19:41:39 UTC).
fn sample() -> MbFib {
    MbFib {
        a: 1853.330707035842,
        b: 1965.611909351887,
        time_ms: 1_754_400_000_000.0,
        levels: [
            1853.330707035842,
            1879.829070943048,
            1896.2221262402613,
            1909.4713081938644,
            1922.720486801227,
            1941.5837335553692,
            1992.110269832543,
        ],
    }
}

/// The ratios Moonbot labels its levels with come back out of the stored prices.
///
/// The oracle is Moonbot's own set — 0, .236, .382, .5, .618, .786, 1.236 — read off the chart that
/// produced this blob, not anything this code computes. It is what makes the labels readable at all,
/// since the object stores no ratios.
///
/// The tolerance is SINGLE-precision, measured and not chosen: the live sample's .236 level comes
/// back as 0.236000001, so Moonbot computes the stored price in `f32` and the error rides back out
/// through the division. Anything tighter would be asserting a precision the format does not have.
#[test]
fn the_ratios_are_recovered_from_the_stored_prices() {
    let f = sample();
    let want = [0.0, 0.236, 0.382, 0.5, 0.618, 0.786, 1.236];
    let span = f.b - f.a;
    for (price, want) in f.levels.iter().zip(want) {
        // The RAW division, before naming: this is what the format actually yields, and asserting
        // the named value alone would compare the snap table with itself.
        let raw = (price - f.a) / span;
        assert!(
            (raw - want).abs() < 1e-6,
            "level at {price} divides to {raw}, not {want}"
        );
        // And the name the label carries, which must be the canonical one exactly.
        assert_eq!(f.ratio_of(*price).expect("the move has height"), want);
    }
}

/// A move with no height yields no ratio instead of an infinity.
///
/// Every ratio is a division by the move's height, and the height comes off the wire. Without this
/// the label layer would be handed `inf` or `NaN` and would draw them as text.
#[test]
fn a_flat_move_has_no_ratios() {
    let mut f = sample();
    f.b = f.a;
    assert_eq!(f.ratio_of(f.a), None);
    assert_eq!(f.ratio_of(f.a + 1.0), None);
}

/// Only levels that are a price get drawn, and the label falls back to the price when the ratio
/// cannot be had.
#[test]
fn unusable_levels_are_not_drawn() {
    let mut f = sample();
    f.levels[1] = f64::NAN;
    f.levels[2] = 0.0;
    f.levels[3] = -5.0;
    let drawn: Vec<f64> = f.drawn().collect();
    assert_eq!(drawn.len(), 4, "a NaN, a zero and a negative are not levels");
    assert!(drawn.iter().all(|p| p.is_finite() && *p > 0.0));
}

/// Stretching from one end keeps the other end still and every level's own ratio.
///
/// The ratios are what a stretch must preserve, prices are what it recomputes — including a level
/// the user had already dragged away from its starting ratio, which must not snap back.
#[test]
fn stretching_one_end_holds_the_other_and_keeps_every_ratio() {
    let mut f = sample();
    // Drag the free level first, so the stretch has a custom ratio to preserve.
    let moved_to = f.levels[4] + 1.0;
    assert!(f.move_handle(2, FigNode::new(0.0, moved_to)));
    let custom = f.ratio_of(moved_to).expect("still measurable");
    assert!(custom != 0.618, "the free level left its starting ratio");

    let before_ratios: Vec<_> = f.levels.iter().map(|p| f.ratio_of(*p)).collect();
    let low = f.handle(0).expect("a low end").price;
    let high = f.handle(1).expect("a high end").price;
    assert!(high > low);

    // Pull the high end up; the low end must not move.
    assert!(f.move_handle(1, FigNode::new(0.0, high + 20.0)));
    assert_eq!(f.handle(0).expect("still there").price, low, "the far end held");
    assert!(
        (f.handle(1).expect("still there").price - (high + 20.0)).abs() < 1e-9,
        "the dragged end landed under the cursor"
    );
    for (p, was) in f.levels.iter().zip(&before_ratios) {
        let now = f.ratio_of(*p);
        match (now, was) {
            (Some(a), Some(b)) => assert!((a - b).abs() < 1e-9, "ratio moved: {a} vs {b}"),
            _ => panic!("a level stopped being measurable"),
        }
    }
}

/// The free level moves alone, and only its own ratio changes.
#[test]
fn the_free_level_moves_by_itself() {
    let mut f = sample();
    let before = f;
    let to = f.levels[4] + 2.0;
    assert!(f.move_handle(2, FigNode::new(0.0, to)));
    assert_eq!(f.a, before.a, "the anchors are untouched");
    assert_eq!(f.b, before.b);
    for (i, (now, was)) in f.levels.iter().zip(before.levels).enumerate() {
        if i == 4 {
            assert_eq!(*now, to);
        } else {
            assert_eq!(*now, was, "level {i} moved with it");
        }
    }
    assert!(!f.move_handle(2, FigNode::new(0.0, to)), "no move is no change");
}

/// The whole figure still travels on a body drag, carrying every stored price.
#[test]
fn the_figure_also_moves_whole() {
    let mut f = sample();
    let before = f;
    assert_eq!(f.handle_count(), 3, "two ends and the free level");
    assert!(f.translate(1000.0, 5.0), "the body drag moves it");
    assert_eq!(f.a, before.a + 5.0);
    assert_eq!(f.b, before.b + 5.0);
    // The instant it was DRAWN, untouched: the figure spans the whole chart, so sideways travel
    // changes nothing visible, and rewriting @64 in another program's object is not a drag's job.
    assert_eq!(f.time_ms, before.time_ms);
    for (after, was) in f.levels.iter().zip(before.levels) {
        assert_eq!(*after, was + 5.0, "every level travels with the figure");
    }
    // The scale is unchanged by the move: the same prices sit at the same ratios.
    for (price, was) in f.levels.iter().zip(before.levels) {
        assert_eq!(f.ratio_of(*price), before.ratio_of(was));
    }
    assert!(!f.translate(0.0, 0.0), "a zero step is not a move");
    assert!(
        !f.translate(5_000.0, 0.0),
        "a purely sideways drag changes nothing and must not re-upsert"
    );
}

/// A fib drawn HERE places its levels at Moonbot's own ratios.
///
/// The set is measured off the wire rather than chosen, and the object stores the resulting PRICES
/// — so a Moonbot whose seventh ratio differs still shows these levels where they were placed, with
/// its own names.
#[test]
fn a_fib_drawn_here_uses_moonbots_ratios() {
    let f = MbFib::spanning(FigNode::new(1_000.0, 100.0), FigNode::new(9_000.0, 200.0));
    assert_eq!(f.a, 100.0);
    assert_eq!(f.b, 200.0);
    assert_eq!(f.time_ms, 1_000.0, "the time it was drawn from, not an edge");
    for (level, ratio) in f.levels.iter().zip(MB_FIB_RATIOS) {
        assert_eq!(*level, 100.0 + ratio * 100.0);
        assert_eq!(f.ratio_of(*level), Some(ratio));
    }
}

/// Slot 0 holds the LOWER anchor whichever way the move was drawn.
///
/// The oracle is the pair of live samples: an upward fib's first slot equals `a` and a downward
/// one's equals `b`. Placing it by ratio alone would put a line where Moonbot has none on every fib
/// drawn downward, and leave none where Moonbot has one.
#[test]
fn a_fib_drawn_downward_puts_its_first_slot_on_the_lower_anchor() {
    let up = MbFib::spanning(FigNode::new(0.0, 100.0), FigNode::new(1.0, 200.0));
    let down = MbFib::spanning(FigNode::new(0.0, 200.0), FigNode::new(1.0, 100.0));
    assert_eq!(up.levels[0], 100.0, "the lower price");
    assert_eq!(down.levels[0], 100.0, "the lower price again");
    assert_eq!(up.ratio_of(up.levels[0]), Some(0.0));
    assert_eq!(down.ratio_of(down.levels[0]), Some(1.0));
    // And the other six sit at the same ratios in both, which is what the samples show.
    for f in [up, down] {
        for (level, ratio) in f.levels.iter().zip(MB_FIB_RATIOS).skip(1) {
            assert_eq!(f.ratio_of(*level), Some(ratio));
        }
    }
}

/// The tool offers no switches: its levels are Moonbot's, and there is nothing here to turn off.
#[test]
fn the_tool_offers_no_level_switches() {
    assert!(sample().settings().is_empty());
}

/// The alert list's Price column asks handle 0, which is the scale's bottom line.
///
/// Not `a`: which anchor is lower depends on the direction the fib was drawn, and a column of
/// The alert list's Price column asks handle 0, which is the scale's bottom END.
///
/// Not `a`, and not the lowest LINE: which anchor is lower depends on the direction the fib was
/// drawn, and an extension can hang below the move without being its bottom.
#[test]
fn the_anchor_price_is_the_bottom_of_the_scale() {
    for f in [
        MbFib::spanning(FigNode::new(0.0, 100.0), FigNode::new(1.0, 200.0)),
        MbFib::spanning(FigNode::new(0.0, 200.0), FigNode::new(1.0, 100.0)),
        down_sample(),
    ] {
        assert_eq!(FigureKind::MbFib(f).anchor_price(), f.a.min(f.b));
    }
}

/// The free level stays grabbable after being dragged past an end.
///
/// Otherwise it would answer for two handles at once, the tie would resolve to the end, and the one
/// level Moonbot lets a user slide could never be slid again.
#[test]
fn the_free_level_stays_its_own_handle_beyond_the_ends() {
    let mut f = sample();
    let above = f.levels[6] + 50.0;
    assert!(f.move_handle(2, FigNode::new(0.0, above)));
    assert_eq!(f.handle(2).expect("still a handle").price, above);
    assert_ne!(
        f.handle(1).expect("an end").price,
        above,
        "the end handle must not become the free level"
    );
    // And it can be slid back.
    assert!(f.move_handle(2, FigNode::new(0.0, above - 10.0)));
}

/// A handle index this tool does not have changes nothing.
#[test]
fn an_unknown_handle_is_refused() {
    let mut f = sample();
    let before = f;
    for i in [3, 9, usize::MAX] {
        assert!(f.handle(i).is_none());
        assert!(!f.move_handle(i, FigNode::new(0.0, 1.0)));
    }
    assert_eq!(f, before);
}

/// The hit test measures the nearest LEVEL, vertically, because every level spans the full width.
#[test]
fn the_nearest_level_is_what_is_grabbed() {
    let f = sample();
    let proj = TestProj;
    // Directly on the middle level: a hit at zero distance regardless of X.
    let on = proj.px_of(FigNode::new(TestProj::T0_MS, f.levels[3]));
    assert!(f.hit((on.0 + 500.0, on.1), &proj) < 0.001);
    // Far above every level: no false grab.
    assert!(f.hit((on.0, on.1 - 400.0), &proj) > 100.0);
}

/// A ratio scale names its levels at REST, not on hover.
///
/// The registry-wide version of this contract only covers tools a gesture can draw, so the one tool
/// that arrives instead asserts it here. A level whose price shows only under the pointer cannot be
/// Moonbot's own picture, at rest: a line at each END of the scale carrying the move as a
/// percentage, the levels between carrying their ratios, the extension above 1 as a line, and the
/// fill stopping at the ends.
///
/// The counts are derived from the model rather than written down: whatever the level set is, the
/// lines are its measurable levels plus the two anchors, and the bands are the gaps inside the move.
#[test]
fn the_picture_matches_moonbots() {
    use crate::figures::tools::tests::{build, ctx};
    for f in [sample(), down_sample()] {
        let (lo, hi) = (f.a.min(f.b), f.a.max(f.b));
        let inner: Vec<f64> = f.drawn().filter(|p| *p != lo && *p != hi).collect();
        let rec = build(&FigureKind::MbFib(f), ctx(false, false));

        assert_eq!(
            rec.hlines.len(),
            inner.len() + 2,
            "every level plus the two ends"
        );
        assert!(rec.hlines.contains(&lo) && rec.hlines.contains(&hi), "the ends are drawn");
        // Every readout is present: a ratio for each level, a percentage for each end.
        assert_eq!(rec.labels.len(), inner.len() + 2, "nothing is left unnamed");

        // The fill stops at the ends: nothing is painted outside the move.
        assert!(!rec.bands.is_empty(), "a scale without bands is not filled");
        for (_, _, p0, p1) in &rec.bands {
            let (band_lo, band_hi) = (p0.min(*p1), p0.max(*p1));
            assert!(
                band_lo >= lo - 1e-9 && band_hi <= hi + 1e-9,
                "band {band_lo}..{band_hi} spills past the move {lo}..{hi}"
            );
        }
        // And an extension outside the move still draws its line.
        for level in f.drawn().filter(|p| *p > hi || *p < lo) {
            assert!(rec.hlines.contains(&level), "the extension keeps its line");
        }
    }
}

/// A level Moonbot could not have drawn is dropped from the picture AND from the hit test, so the
/// two cannot disagree about where the figure is.
#[test]
fn an_unusable_level_is_absent_from_both_the_picture_and_the_hit_test() {
    let mut f = sample();
    f.levels[6] = f64::MAX; // finite in f64, an infinity once the buffers narrow to f32
    assert_eq!(f.drawn().count(), 6);
    let proj = TestProj;
    let far = proj.px_of(FigNode::new(TestProj::T0_MS, f.levels[6]));
    assert!(
        f.hit(far, &proj) > 100.0,
        "a level that is not drawn must not be grabbable"
    );
}

/// The level that sits ON an anchor draws its line and carries no name.
///
/// Which anchor it is flips with the direction the fib was drawn — measured on live samples, an
/// upward one puts it on `a` and a downward one on `b` — so the same slot would be named 0 in one
/// A level sitting exactly ON an anchor is one line, not two, and it reads as the anchor.
///
/// Which anchor that is flips with the direction the fib was drawn — an upward one puts it on `a`,
/// a downward one on `b` — so naming it by its ratio would print 0 for one and 1 for the other.
/// Moonbot prints neither: it shows the move as a percentage there.
#[test]
fn a_level_on_an_anchor_becomes_the_anchor_line() {
    use crate::figures::tools::tests::{build, ctx};
    for f in [sample(), down_sample()] {
        let (lo, hi) = (f.a.min(f.b), f.a.max(f.b));
        let rec = build(&FigureKind::MbFib(f), ctx(false, false));
        assert_eq!(
            rec.hlines.iter().filter(|p| **p == lo).count(),
            1,
            "the level on the anchor is not drawn twice"
        );
        assert_eq!(rec.hlines.iter().filter(|p| **p == hi).count(), 1);
    }
}

/// The scale is filled between its levels, in the levels' own hues.
///
/// Moonbot draws those bands and sends no fill — every byte of the 145 is accounted for — so they
/// come from the same palette our own Fibonacci uses, read by the ratio each level recovers to.
/// Ordered by PRICE and not by slot: a fib drawn downward writes its anchor level first, so banding
/// the array as stored would join levels that are not neighbours on the chart.
#[test]
fn the_scale_is_filled_between_its_levels_in_their_own_hues() {
    use crate::figures::tools::tests::{build, ctx};
    for f in [sample(), down_sample()] {
        let rec = build(&FigureKind::MbFib(f), ctx(false, false));
        assert!(!rec.bands.is_empty(), "a scale without bands is not filled");
        for (_, _, p0, p1) in &rec.bands {
            let (lo, hi) = (p0.min(*p1), p0.max(*p1));
            // Every band joins two ADJACENT drawn levels: nothing else may lie between them.
            let between = f
                .drawn()
                .filter(|p| *p > lo + 1e-9 && *p < hi - 1e-9)
                .count();
            assert_eq!(between, 0, "band {lo}..{hi} skips a level");
        }
        // And no band is painted in the figure's own colour: the hues belong to the scale.
        for c in &rec.band_colors {
            assert!(
                c[..3] != [1.0, 1.0, 1.0],
                "a band took the stroke colour instead of its level's hue"
            );
        }
    }
}
