use super::*;
use crate::figures::kind::FigureKind;
use crate::figures::tools::tests::TestProj;

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

/// The object is never editable by a drag, whichever handle a caller asks for.
#[test]
fn moonbots_object_takes_no_edit() {
    let mut f = sample();
    let before = f;
    assert_eq!(f.handle_count(), 0);
    assert!(!f.move_handle(0, FigNode::new(1.0, 2.0)));
    assert!(!f.translate(1000.0, 5.0));
    assert_eq!(f, before);
}

/// The alert list's Price column asks index 0 directly, past the zero handle count.
#[test]
fn the_anchor_price_is_still_answered() {
    let f = sample();
    assert_eq!(FigureKind::MbFib(f).anchor_price(), f.a);
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
/// read at a glance, which is the whole purpose of the object.
#[test]
fn the_levels_are_named_on_an_idle_chart() {
    use crate::figures::tools::tests::{build, ctx};
    let rec = build(&FigureKind::MbFib(sample()), ctx(false, false));
    assert_eq!(rec.labels.len(), 7, "every level names itself at rest");
    assert_eq!(rec.hlines.len(), 7, "and draws across the whole chart");
    assert!(rec.handles.is_empty(), "and offers no drag knot");
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
