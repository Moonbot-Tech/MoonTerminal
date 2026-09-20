// Explicit imports, never `use super::*`: the crate's views import `gpui::*`, whose own
// `test` shadows the built-in attribute and makes `#[test]` expand recursively.
use super::super::super::state::TapeStatus;
use super::{retry_wait, split_by_key};
use moon_core::market::trade_replay::TickStatus;

/// Only a gate refusal on a still-uncovered row is asked again, with the gate's own number;
/// a row the tiles covered anyway, and every other status, is final here (a venue's own
/// refusal gets its one retry in the thread, not in this rule).
#[test]
fn only_a_refused_uncovered_row_waits_the_gate_out() {
    let refused = TickStatus::RateLimited { retry_in_s: 27 };
    assert_eq!(retry_wait(refused, TapeStatus::Missing), Some(27));
    assert_eq!(retry_wait(refused, TapeStatus::Covered), None);
    assert_eq!(retry_wait(TickStatus::Failed, TapeStatus::Missing), None);
    assert_eq!(retry_wait(TickStatus::NoRoute, TapeStatus::Missing), None);
    assert_eq!(retry_wait(TickStatus::Served, TapeStatus::Missing), None);
}

/// A deferral takes every queued row of the refused venue, in queue order, and leaves the
/// other venues' rows in theirs.
#[test]
fn a_deferral_takes_the_venues_rows_and_keeps_the_rest_in_order() {
    let rows = vec![
        (1, "binance"),
        (2, "gate"),
        (3, "binance"),
        (4, "okx"),
        (5, "gate"),
    ];
    let (same, other) = split_by_key(rows, "gate", |r| r.1);
    assert_eq!(same, vec![(2, "gate"), (5, "gate")]);
    assert_eq!(other, vec![(1, "binance"), (3, "binance"), (4, "okx")]);
    let (none, all) = split_by_key(vec![(1, "binance")], "", |r| r.1);
    assert!(none.is_empty(), "a row of no venue never joins a wait");
    assert_eq!(all, vec![(1, "binance")]);
}
