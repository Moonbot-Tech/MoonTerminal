//! Unit tests for the ladder's tick, step and precision choices.

use super::*;

#[test]
fn the_step_is_the_smallest_whose_row_fits_the_text() {
    // Tick 0.01 at 100 px per price unit: one tick is 1 px, so a 15 px line needs 20 ticks.
    assert_eq!(row_step(0.01, 100.0, 15.0), Some(20));
    // At 2000 px per unit one tick is 20 px — a single tick already fits.
    assert_eq!(row_step(0.01, 2000.0, 15.0), Some(1));
    // At 400 px per unit a tick is 4 px: 4 ticks make 16 px.
    assert_eq!(row_step(0.01, 400.0, 15.0), Some(4));
}

#[test]
fn an_unusable_grid_or_a_hopeless_zoom_falls_back_to_the_glass() {
    assert_eq!(row_step(0.0, 100.0, 15.0), None);
    assert_eq!(row_step(f64::NAN, 100.0, 15.0), None);
    assert_eq!(row_step(0.01, 0.0, 15.0), None);
    // A tick of 1e-8 at 1 px per unit would need ~1.5e9 ticks a row.
    assert_eq!(row_step(1e-8, 1.0, 15.0), None);
}

#[test]
fn a_stored_highlight_snaps_onto_its_list() {
    assert_eq!(snap_big_pct(60), 50);
    assert_eq!(snap_big_pct(200), 75);
}

#[test]
fn prices_print_with_the_tick_s_own_decimals() {
    assert_eq!(tick_decimals(0.00001), 5);
    assert_eq!(tick_decimals(0.5), 1);
    assert_eq!(tick_decimals(0.0025), 4);
    assert_eq!(tick_decimals(10.0), 0);
}

#[test]
fn the_exchange_tick_wins_and_is_not_asked_for_again() {
    let mut t = LadderTick::default();
    assert!(t.due(0));
    t.record(0, Some(0.01), || {
        panic!("the book is not read once the exchange tick is known")
    });
    assert_eq!(t.tick(), Some(0.01));
    assert!(!t.due(10_000));
}

#[test]
fn an_unknown_tick_is_asked_for_at_most_once_a_retry_period() {
    let mut t = LadderTick::default();
    t.record(1_000, None, || None);
    assert!(!t.due(1_000 + TICK_RETRY_MS - 1));
    assert!(t.due(1_000 + TICK_RETRY_MS));
}

#[test]
fn the_book_s_tick_only_ever_gets_finer_until_the_exchange_s_arrives() {
    let mut t = LadderTick::default();
    t.record(0, None, || Some(0.01));
    t.record(1_000, None, || Some(0.1));
    assert_eq!(t.tick(), Some(0.01));
    t.record(2_000, None, || Some(0.001));
    assert_eq!(t.tick(), Some(0.001));
    t.record(3_000, Some(0.005), || None);
    assert_eq!(t.tick(), Some(0.005));
}

#[test]
fn an_unusable_tick_counts_as_none() {
    let mut t = LadderTick::default();
    t.record(0, Some(0.0), || Some(f64::NAN));
    assert_eq!(t.tick(), None);
}
