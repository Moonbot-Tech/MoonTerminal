//! Unit tests for the bubble size scale and the label fit.

use super::*;

#[test]
fn the_size_runs_from_the_smallest_disc_at_ten_to_the_largest_at_a_hundred_thousand() {
    // f32 `log10` can land a hair off the exact decade, so the ends are checked to a tolerance.
    assert!((radius_frac(10.0) - BUBBLE_MIN_FRAC).abs() < 1e-5);
    assert!((radius_frac(100_000.0) - 1.0).abs() < 1e-5);
    // Halfway in log terms — 1000 — is halfway between the two radii.
    let mid = radius_frac(1_000.0);
    assert!(
        (mid - (BUBBLE_MIN_FRAC + 1.0) * 0.5).abs() < 1e-6,
        "mid {mid}"
    );
}

#[test]
fn amounts_outside_the_scale_clamp_and_nonsense_reads_as_the_smallest() {
    assert_eq!(radius_frac(1.0), BUBBLE_MIN_FRAC);
    assert_eq!(radius_frac(1e9), 1.0);
    assert_eq!(radius_frac(0.0), BUBBLE_MIN_FRAC);
    assert_eq!(radius_frac(-5.0), BUBBLE_MIN_FRAC);
    assert_eq!(radius_frac(f32::NAN), BUBBLE_MIN_FRAC);
    // The shaders' `n > 0.0` passes +inf and saturate it to the largest disc; so does this.
    assert_eq!(radius_frac(f32::INFINITY), 1.0);
}

#[test]
fn only_bubbles_widen_the_marker_extent() {
    let crosses = ChartGraphicsCfg::default();
    assert_eq!(marker_extent_scale(&crosses), crosses.marker_scale);
    let bubbles = ChartGraphicsCfg {
        trade_kind: TradeKind::Bubbles,
        ..crosses
    };
    assert_eq!(
        marker_extent_scale(&bubbles),
        crosses.marker_scale * BUBBLE_MAX_MULT
    );
}

#[test]
fn a_label_fits_only_a_disc_that_holds_its_corners() {
    // A 20×10 label needs a radius of sqrt(11² + 6²) ≈ 12.5.
    assert!(label_fits(13.0, 20.0, 10.0));
    assert!(!label_fits(12.0, 20.0, 10.0));
}

#[test]
fn the_notional_for_a_size_inverts_the_size_for_a_notional() {
    let n = notional_for_frac(radius_frac(2_500.0)).expect("a reachable size");
    assert!((n - 2_500.0).abs() / 2_500.0 < 1e-3, "n {n}");
    assert_eq!(notional_for_frac(BUBBLE_MIN_FRAC * 0.5), Some(0.0));
    assert_eq!(notional_for_frac(1.01), None);
    assert_eq!(notional_for_frac(f32::NAN), None);
}
