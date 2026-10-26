//! Unit tests for the ladder's bucketing: which row a level lands in and what the row sums.

use super::*;
use crate::feed::{Level, OrderBook};

fn lvl(price: f32, qty: f32) -> Level {
    Level { price, qty }
}

fn model(bids: Vec<Level>, asks: Vec<Level>) -> OrderBookModel {
    let mut model = OrderBookModel::default();
    model.update(&OrderBook { bids, asks });
    model
}

fn rows(model: &OrderBookModel, tick: f64, step: u32, lo: f32, hi: f32) -> Vec<LadderRow> {
    let mut out = Vec::new();
    model.build_ladder(LadderGrid { tick, step }, lo, hi, &mut out);
    out
}

#[test]
fn bids_round_down_and_asks_round_up_into_their_rows() {
    // Two ticks a row: the bid at 99.99 joins 99.98's row, the ask at 100.01 joins 100.02's.
    let m = model(
        vec![lvl(100.00, 1.0), lvl(99.99, 1.0), lvl(99.98, 1.0)],
        vec![lvl(100.01, 1.0), lvl(100.02, 1.0), lvl(100.03, 1.0)],
    );
    let got = rows(&m, 0.01, 2, 0.0, 1000.0);
    // One unit per level, so a row's notional is the sum of its prices.
    let summary: Vec<(i64, i64, bool)> = got
        .iter()
        .map(|r| {
            (
                (r.price * 100.0).round() as i64,
                (f64::from(r.notional) * 100.0).round() as i64,
                r.is_ask,
            )
        })
        .collect();
    assert_eq!(
        summary,
        vec![
            (10000, 10000, false),
            (9998, 9999 + 9998, false),
            (10002, 10001 + 10002, true),
            (10004, 10003, true),
        ]
    );
}

#[test]
fn a_price_carrying_f32_noise_stays_on_its_own_tick() {
    // 2.0664 is 2.06640005 in f32; dividing it straight by the tick and taking the ceiling would
    // put the ask one row up, at 2.06641.
    let m = model(vec![], vec![lvl(2.0664, 3.0)]);
    let got = rows(&m, 0.00001, 1, 0.0, 10.0);
    assert_eq!(got.len(), 1);
    assert!(
        (got[0].price - 2.0664).abs() < 1e-9,
        "row at {}",
        got[0].price
    );
}

#[test]
fn a_row_sums_the_notional_of_its_levels() {
    // Bids round DOWN, so 19 and 12 share the row at 10 (tick 1, ten ticks a row).
    let m = model(vec![lvl(19.0, 2.0), lvl(12.0, 1.0)], vec![]);
    let got = rows(&m, 1.0, 10, 0.0, 100.0);
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].price, 10.0);
    assert_eq!(got[0].notional, 50.0);
}

#[test]
fn rows_outside_the_window_are_left_out() {
    let m = model(vec![lvl(100.0, 1.0), lvl(90.0, 1.0)], vec![lvl(110.0, 1.0)]);
    let got = rows(&m, 1.0, 1, 95.0, 105.0);
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].price, 100.0);
}

#[test]
fn an_unusable_grid_yields_no_rows() {
    let m = model(vec![lvl(100.0, 1.0)], vec![lvl(101.0, 1.0)]);
    assert!(rows(&m, 0.0, 1, 0.0, 1000.0).is_empty());
    assert!(rows(&m, f64::NAN, 1, 0.0, 1000.0).is_empty());
    assert!(rows(&m, 0.01, 0, 0.0, 1000.0).is_empty());
}

#[test]
fn the_biggest_visible_row_gets_the_full_bar_and_the_margin_does_not_set_the_scale() {
    let rows = vec![
        LadderRow {
            price: 100.0,
            notional: 50.0,
            is_ask: false,
        },
        LadderRow {
            price: 101.0,
            notional: 25.0,
            is_ask: true,
        },
        // In the emitted margin only: bigger, but not on screen.
        LadderRow {
            price: 120.0,
            notional: 500.0,
            is_ask: true,
        },
    ];
    let mut inst = Vec::new();
    let max = ladder_instances(&rows, 99.0, 102.0, &mut inst);
    assert_eq!(max, 50.0);
    let lens: Vec<f32> = inst.iter().map(|i| i.len_norm).collect();
    assert_eq!(lens, vec![1.0, 0.5, 1.0]);
    let kinds: Vec<f32> = inst.iter().map(|i| i.kind).collect();
    assert_eq!(kinds, vec![2.0, 3.0, 3.0]);
}

#[test]
fn the_tick_is_read_back_from_f32_prices_through_their_noise() {
    // Five ticks of 0.00001 around 2.0664: the f32 gaps come out near, not at, the tick.
    let m = model(
        vec![lvl(2.0664, 1.0), lvl(2.06639, 1.0), lvl(2.06638, 1.0)],
        vec![lvl(2.06641, 1.0), lvl(2.06642, 1.0)],
    );
    let tick = m.infer_tick().expect("a tick");
    assert!((tick - 0.00001).abs() < 1e-12, "tick {tick}");
}

#[test]
fn a_quarter_grid_is_inferred_as_a_quarter() {
    let m = model(vec![lvl(10.0, 1.0), lvl(9.9975, 1.0)], vec![]);
    let tick = m.infer_tick().expect("a tick");
    assert!((tick - 0.0025).abs() < 1e-12, "tick {tick}");
}

#[test]
fn a_sparse_book_keeps_every_level_at_its_own_price() {
    // Levels three ticks of 0.01 apart. A grid of 0.02 or 0.025 would move 99.97 into a
    // neighbour's row; the inferred grid must be one every level already sits on.
    let m = model(
        vec![lvl(100.0, 1.0), lvl(99.97, 1.0), lvl(99.94, 1.0)],
        vec![],
    );
    let tick = m.infer_tick().expect("a tick");
    assert!((tick - 0.01).abs() < 1e-12, "tick {tick}");
    let prices: Vec<i64> = rows(&m, tick, 1, 0.0, 1000.0)
        .iter()
        .map(|r| (r.price * 100.0).round() as i64)
        .collect();
    assert_eq!(prices, vec![10000, 9997, 9994]);
}

#[test]
fn an_exact_five_grid_is_inferred_as_itself() {
    let m = model(vec![lvl(100.0, 1.0), lvl(99.5, 1.0)], vec![]);
    let tick = m.infer_tick().expect("a tick");
    assert!((tick - 0.5).abs() < 1e-12, "tick {tick}");
}

#[test]
fn a_book_without_two_levels_on_a_side_infers_nothing() {
    // One bid and one ask: the spread is not a tick.
    let m = model(vec![lvl(100.0, 1.0)], vec![lvl(101.0, 1.0)]);
    assert_eq!(m.infer_tick(), None);
}

#[test]
fn levels_several_half_ticks_apart_keep_a_grid_they_sit_on() {
    // Tick 0.5, levels three ticks apart: the smallest gap reads 1.5, whose decade is 1.0. On a
    // 1.0 grid 100.5 rounds to 101 and 103.5 to 104, two prices nobody quoted; every level is a
    // whole number of 0.5 (201, 204 and 207 of them), so the grid must divide 0.5.
    let m = model(
        vec![lvl(103.5, 1.0), lvl(102.0, 1.0), lvl(100.5, 1.0)],
        vec![],
    );
    let tick = m.infer_tick().expect("a tick");
    assert!(
        ((0.5 / tick) - (0.5 / tick).round()).abs() < 1e-9,
        "tick {tick} is not a divisor of 0.5"
    );
    let prices: Vec<i64> = rows(&m, tick, 1, 0.0, 1000.0)
        .iter()
        .map(|r| (r.price * 10.0).round() as i64)
        .collect();
    assert_eq!(prices, vec![1035, 1020, 1005]);
}

#[test]
fn a_quarter_tick_two_apart_is_not_widened_to_half_a_cent() {
    // Tick 0.0025, levels two ticks apart: the gap 0.005 snaps to 0.005 exactly, and 1.0025 is
    // 200.5 of those, so it would round onto 1.005 or 1.0. The grid must divide 0.0025.
    let m = model(vec![lvl(1.0075, 1.0), lvl(1.0025, 1.0)], vec![]);
    let tick = m.infer_tick().expect("a tick");
    assert!(
        ((0.0025 / tick) - (0.0025 / tick).round()).abs() < 1e-6,
        "tick {tick} is not a divisor of 0.0025"
    );
}
