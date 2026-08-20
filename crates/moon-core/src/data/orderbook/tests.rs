//! Unit tests for the whole-book depth figures the chart's sell-line labels read.

use super::*;
use crate::feed::{Level, OrderBook};

fn lvl(price: f32, qty: f32) -> Level {
    Level { price, qty }
}

/// Book with a 100/101 spread: bids 100, 99, 98 and asks 101, 102, 103, one unit each, so each
/// level's notional equals its price.
fn book() -> OrderBookModel {
    let mut model = OrderBookModel::default();
    model.update(&OrderBook {
        bids: vec![lvl(100.0, 1.0), lvl(99.0, 1.0), lvl(98.0, 1.0)],
        asks: vec![lvl(101.0, 1.0), lvl(102.0, 1.0), lvl(103.0, 1.0)],
    });
    model
}

#[test]
fn asks_below_the_price_are_summed_for_a_long() {
    // A long's sell line at 103 clears the asks at 101 and 102.
    assert_eq!(book().side_notional_toward(103.0, true), Some(203.0));
}

#[test]
fn bids_above_the_price_are_summed_for_a_short() {
    // A short's sell line at 98 clears the bids at 100 and 99.
    assert_eq!(book().side_notional_toward(98.0, false), Some(199.0));
}

#[test]
fn the_level_at_the_price_itself_is_excluded() {
    // Strict comparison on both sides: the line's own level is not glass to clear.
    assert_eq!(book().side_notional_toward(102.0, true), Some(101.0));
    assert_eq!(book().side_notional_toward(99.0, false), Some(100.0));
}

#[test]
fn the_opposite_side_never_contributes() {
    // Asks sit above the spread, so no ask lies below a price under it, and vice versa. The side
    // is present, so the answer is a known zero rather than "unknown".
    assert_eq!(book().side_notional_toward(99.0, true), Some(0.0));
    assert_eq!(book().side_notional_toward(102.0, false), Some(0.0));
}

#[test]
fn the_figure_ignores_any_visible_window() {
    // The regression this method exists for: the caller used to sum `collect_visible_depth`
    // output, so a chart showing only part of the span reported only part of the glass. The same
    // question asked of the whole book answers with the market, not with the camera.
    let model = book();
    let mut visible = Vec::new();
    model.collect_visible_depth(102.5, 103.5, &mut visible);
    let from_visible: f32 = visible
        .iter()
        .filter(|l| l.is_ask && l.price < 103.0)
        .map(|l| l.notional)
        .sum();
    assert!(from_visible < 203.0, "window must clip the old figure");
    assert_eq!(model.side_notional_toward(103.0, true), Some(203.0));
}

#[test]
fn a_non_finite_level_does_not_poison_the_sum() {
    // Quantity and price arrive over the wire; one bad level must not turn every sell-line
    // volume on the chart into "NaN".
    let mut model = OrderBookModel::default();
    model.update(&OrderBook {
        bids: Vec::new(),
        asks: vec![lvl(101.0, 1.0), lvl(102.0, f32::NAN)],
    });
    assert_eq!(model.side_notional_toward(103.0, true), Some(101.0));
}

#[test]
fn an_empty_book_reports_an_unknown_side() {
    let model = OrderBookModel::default();
    assert_eq!(model.side_notional_toward(100.0, true), None);
    assert_eq!(model.side_notional_toward(100.0, false), None);
}

#[test]
fn a_one_sided_book_leaves_the_missing_side_unknown() {
    let mut model = OrderBookModel::default();
    model.update(&OrderBook {
        bids: Vec::new(),
        asks: vec![lvl(101.0, 2.0), lvl(102.0, 1.0)],
    });
    assert_eq!(model.side_notional_toward(103.0, true), Some(304.0));
    assert_eq!(model.side_notional_toward(100.0, false), None);
}
