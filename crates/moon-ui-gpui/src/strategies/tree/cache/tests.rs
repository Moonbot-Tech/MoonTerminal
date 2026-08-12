//! Unit tests for the tree-cache digest.
//!
//! Explicit imports, never `use super::*`: the parent re-exports `gpui::*`, whose own `test`
//! shadows the built-in attribute and makes `#[test]` expand recursively.

use moon_core::feed::OrderRow;
use moon_core::session::store::CoreData;

use super::{open_orders_digest, unordered};

/// A `HashSet` hands its elements over in whatever order it likes, and that order changes between
/// runs of a program that inserted the same values. Folding it as a sequence would therefore
/// produce a fresh signature for identical state — the cache would miss every frame and the window
/// would be exactly as slow as before, while looking correct.
#[test]
fn the_digest_ignores_the_order_it_is_given() {
    let forward = unordered([1u64, 2, 3, 4].iter());
    let shuffled = unordered([3u64, 1, 4, 2].iter());
    assert_eq!(forward, shuffled);
}

/// The digest still has to MOVE when the contents do, or the cache serves a stale tree forever.
#[test]
fn the_digest_moves_when_an_element_changes() {
    let before = unordered([1u64, 2, 3].iter());
    assert_ne!(before, unordered([1u64, 2, 4].iter()));
}

/// Length is mixed in separately, so growing or shrinking a set is visible even when the sum of
/// the element hashes would not be — an empty set and a set holding a zero-hashing value are not
/// the same state.
#[test]
fn the_digest_separates_sets_of_different_size() {
    let one = unordered([7u64].iter());
    assert_ne!(one, unordered([7u64, 7].iter()));
    assert_ne!(unordered(std::iter::empty::<&u64>()), one);
}

/// One open order on `strat_id`, priced at `price`.
fn order(strat_id: u64, job_is_done: bool, price: f32) -> OrderRow {
    OrderRow {
        market: "BTCUSDT".into(),
        market_display: "BTCUSDT".into(),
        coin: "BTC".into(),
        quote: "USDT".into(),
        is_short: false,
        size: 0.01,
        remaining_size: 0.01,
        sl_on: false,
        ts_on: false,
        vstop_on: false,
        sl_fixed: false,
        ts_fixed: false,
        vstop_fixed: false,
        vstop_level: 0.0,
        vstop_vol: 0.0,
        buy_price: 60_000.0,
        sell_price: 0.0,
        create_time_ms: 1_000.0,
        sell_create_time_ms: 0.0,
        entry_fill_time_ms: 0.0,
        price,
        fill_pct: 0.0,
        strat: "test".into(),
        strat_name: String::new(),
        strat_id,
        status: String::new(),
        uid: strat_id,
        emulator: false,
        job_is_done,
        pending: false,
        filled: false,
        stop_loss: None,
        trailing: None,
        take_profit: None,
        vstop: None,
        pending_cond: None,
        liq: None,
        panic_sell: false,
        is_moon_shot: false,
        corridor_price_down: 0.0,
        corridor_price_up: 0.0,
        buy_trace: None,
        sell_trace: None,
    }
}

/// Builds a core holding exactly `orders`.
fn core_with(orders: Vec<OrderRow>) -> CoreData {
    let mut cd = CoreData::default();
    cd.orders = orders;
    cd
}

/// The reason this digest exists instead of `orders_table_rev`: a price tick on an untouched order
/// advances that revision, and keying the cache on it made a live account miss on ~45% of its
/// frames and rebuild an identical tree. The tree renders open-order COUNTS, and a moving price
/// changes none of them.
#[test]
fn a_price_tick_on_an_open_order_leaves_the_digest_alone() {
    let before = open_orders_digest(&core_with(vec![order(7, false, 60_000.0)]));
    let after = open_orders_digest(&core_with(vec![order(7, false, 61_500.0)]));
    assert_eq!(before, after);
}

/// The other half of the same trade-off: what the tree DOES render must move the digest, or the
/// caption keeps a count the core has already retired. This is the stale-tree failure the cache
/// makes possible, so it is the one worth a test.
#[test]
fn an_order_finishing_moves_the_digest() {
    let open = open_orders_digest(&core_with(vec![order(7, false, 60_000.0)]));
    let done = open_orders_digest(&core_with(vec![order(7, true, 60_000.0)]));
    assert_ne!(
        open, done,
        "a finished order leaves the core and strategy counts, so the tree must rebuild"
    );
}

/// The digest carries WHICH strategy each open order belongs to, not just how many there are:
/// the count sits beside the strategy row, so one order moving between two strategies changes two
/// captions while the total stays put.
#[test]
fn moving_an_order_between_strategies_moves_the_digest() {
    let on_seven = open_orders_digest(&core_with(vec![order(7, false, 60_000.0)]));
    let on_eight = open_orders_digest(&core_with(vec![order(8, false, 60_000.0)]));
    assert_ne!(on_seven, on_eight);
}

/// Finished orders are invisible to the tree, so a core accumulating them must not keep
/// invalidating the cache — this is the steady state of any account that has traded for a while.
#[test]
fn finished_orders_do_not_contribute() {
    let just_open = open_orders_digest(&core_with(vec![order(7, false, 60_000.0)]));
    let open_plus_history = open_orders_digest(&core_with(vec![
        order(7, false, 60_000.0),
        order(9, true, 55_000.0),
        order(11, true, 51_000.0),
    ]));
    assert_eq!(just_open, open_plus_history);
}

/// A second open order on the SAME strategy changes the count that strategy's row shows.
#[test]
fn a_second_open_order_on_one_strategy_moves_the_digest() {
    let one = open_orders_digest(&core_with(vec![order(7, false, 60_000.0)]));
    let two = open_orders_digest(&core_with(vec![
        order(7, false, 60_000.0),
        order(7, false, 60_100.0),
    ]));
    assert_ne!(one, two);
}
