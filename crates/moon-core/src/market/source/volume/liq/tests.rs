use super::*;

/// A window centred on a moment reaches equally either side of it.
///
/// Breakage: a trailing window would answer for the run-up to a spike and leave the spike itself
/// out — which is the opposite of what a reader pointing AT the spike is asking.
#[test]
fn a_centred_window_spans_both_sides_of_the_point() {
    let span = VolumeSpan::Millis(60_000);
    let (from, to) = VolumeAt::Around(1_000_000)
        .bounds(span, 9_999_999)
        .expect("bounds");
    assert_eq!(from, 1_000_000 - 30_000);
    assert_eq!(to, 1_000_000 + 30_000);
    assert_eq!(to - from, 60_000, "the window is the period it names");
}

/// An odd period still spans exactly its own length.
///
/// Breakage: halving twice loses the odd millisecond, and a caption saying `1с` would measure 999.
#[test]
fn an_odd_period_keeps_its_length() {
    let span = VolumeSpan::Millis(1_001);
    let (from, to) = VolumeAt::Around(500).bounds(span, 0).expect("bounds");
    assert_eq!(to - from, 1_001);
}

/// The live edge ends the window now.
#[test]
fn the_live_anchor_ends_at_now() {
    let span = VolumeSpan::Millis(5_000);
    let (from, to) = VolumeAt::Now.bounds(span, 1_000_000).expect("bounds");
    assert_eq!((from, to), (995_000, 1_000_000));
}

/// A trade COUNT describes no window, at either anchor.
///
/// Breakage: the rings are read forward from a cursor, so "the N trades before this moment" cannot
/// be answered cheaply. Inventing a window for it would print a figure for a different question.
#[test]
fn a_trade_count_is_not_a_window() {
    let span = VolumeSpan::Trades(500);
    assert!(VolumeAt::Now.bounds(span, 0).is_none());
    assert!(VolumeAt::Around(1_000).bounds(span, 0).is_none());
}
