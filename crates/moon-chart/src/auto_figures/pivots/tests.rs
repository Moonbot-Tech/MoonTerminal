use super::super::test_bars::{bar, from_hl};
use super::*;

fn kinds_at(pivots: &[Pivot]) -> Vec<(usize, PivotKind)> {
    pivots.iter().map(|p| (p.idx, p.kind)).collect()
}

/// A peak and a trough with one bar each side are found, and nothing in the last `right` bars.
///
/// Breakage: an off-by-one on either window edge, or confirming a pivot without its right bars.
#[test]
fn finds_a_peak_and_a_trough_and_never_looks_ahead() {
    let bars = from_hl(&[
        (10.0, 9.0),
        (12.0, 11.0),
        (10.0, 9.0),
        (8.0, 7.0),
        (10.0, 9.0),
    ]);
    let mut out = Vec::new();
    find_pivots(&bars, 1, 1, &mut out);
    assert_eq!(
        kinds_at(&out),
        vec![(1, PivotKind::High), (3, PivotKind::Low)]
    );
    assert_eq!(out[0].price, 12.0);
    assert_eq!(out[1].price, 7.0);

    // The same trough as the LAST bar is not a pivot: its right neighbour does not exist yet.
    find_pivots(&bars[..4], 1, 1, &mut out);
    assert_eq!(kinds_at(&out), vec![(1, PivotKind::High)]);
}

/// A flat top of equal highs is one pivot, at its first bar.
///
/// Breakage: `<=` on both sides (no pivot at all) or `<` on both (one per bar), either of which
/// miscounts touches downstream.
#[test]
fn a_plateau_is_one_pivot() {
    let bars = from_hl(&[(10.0, 9.0), (12.0, 11.0), (12.0, 11.0), (10.0, 9.0)]);
    let mut out = Vec::new();
    find_pivots(&bars, 1, 1, &mut out);
    let highs: Vec<usize> = out
        .iter()
        .filter(|p| p.kind == PivotKind::High)
        .map(|p| p.idx)
        .collect();
    assert_eq!(highs, vec![1]);
}

/// Degenerate windows: too few bars, zero widths, and a NaN neighbour.
///
/// Breakage: an underflow in `n - r` on short input, or a NaN neighbour compared as "lower".
#[test]
fn degenerate_inputs_are_safe() {
    let mut out = Vec::new();
    find_pivots(&[], 3, 3, &mut out);
    assert!(out.is_empty());
    find_pivots(&from_hl(&[(1.0, 0.5); 6]), 3, 3, &mut out);
    assert!(out.is_empty(), "6 bars cannot hold a 3/3 pivot");

    let two = from_hl(&[(10.0, 9.0), (11.0, 8.0)]);
    find_pivots(&two, 0, 0, &mut out);
    assert_eq!(out.len(), 4, "every bar is both a high and a low pivot");

    let mut nan = from_hl(&[(10.0, 9.0), (12.0, 11.0), (10.0, 9.0)]);
    nan[2] = bar(2, f32::NAN, f32::NAN, f32::NAN, f32::NAN);
    find_pivots(&nan, 1, 1, &mut out);
    assert!(out.is_empty(), "a NaN neighbour confirms nothing");
}
