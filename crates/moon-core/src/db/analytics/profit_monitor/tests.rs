//! Regression tests for the compact Profit Monitor aggregate.

use rusqlite::Connection;

use super::super::Query;
use super::profit_monitor_on;
use crate::db::{ProfitScope, SideFilter};

/// Build the unfiltered real-trade monitor query used by this module.
///
/// Args:
///     from: Inclusive lower Unix-second bound.
///     to: Exclusive upper Unix-second bound.
///
/// Returns:
///     A quote-profit query over all cores and strategies.
fn query(from: i64, to: i64) -> Query {
    Query {
        time_zone: chrono_tz::UTC,
        previous_period_basis: Default::default(),
        from,
        to,
        cores: Vec::new(),
        side: SideFilter::All,
        emulator: Some(false),
        strategies: Vec::new(),
        metric: Default::default(),
        valuation: Default::default(),
    }
}

/// Build a report replica with two cores and independently calculated additive metrics.
///
/// Returns:
///     An in-memory report database whose latest first-core name is `alpha-new`.
fn fixture() -> Connection {
    let connection = Connection::open_in_memory().expect("in-memory report database");
    connection
        .execute_batch(
            r#"CREATE TABLE orders_rep(
                core_uid INTEGER, core_name TEXT, coin TEXT, isshort INTEGER,
                buydate INTEGER, closedate INTEGER, profitbtc REAL, strategyid INTEGER,
                emulator INTEGER, spentbtc REAL, basecurrency INTEGER
            );
            INSERT INTO orders_rep VALUES
                (1, 'alpha-old', 'BTC', 0, 90, 100, 12.0, 1, 0, 120.0, 1),
                (2, 'beta',      'ETH', 0, 91, 110, -5.0, 2, 0, 200.0, 1),
                (1, 'alpha-new', 'SOL', 0, 92, 120,  3.0, 3, 0,   0.0, 1),
                (1, '',          'XRP', 0, 93, 130, -2.0, 4, 0,  80.0, 1);"#,
        )
        .expect("profit monitor fixture schema");
    connection
}

/// `profit_monitor.rs:profit_monitor_on` must count only positive spends in the average-order
/// denominator; changing its conditional SQL `COUNT` to `COUNT(*)` makes the named average-order
/// assertion red and shows a false order size in the desktop widget.
#[test]
fn aggregates_exact_per_core_metrics_and_latest_nonblank_name() {
    let connection = fixture();
    let scoped = profit_monitor_on(&connection, &query(0, 200)).expect("monitor aggregate");
    let ProfitScope::Comparable { data, .. } = scoped else {
        panic!("single-quote fixture must be comparable");
    };

    assert_eq!(data.cores.len(), 2);
    let alpha = &data.cores[0];
    assert_eq!(alpha.core_uid, 1);
    assert_eq!(alpha.report_name, "alpha-new");
    assert_eq!(alpha.trades, 3);
    assert_eq!(alpha.wins, 2);
    assert!((alpha.profit - 13.0).abs() < 1e-9);
    assert!((alpha.win_rate() - 200.0 / 3.0).abs() < 1e-9);
    assert!((alpha.average_order() - 100.0).abs() < 1e-9);

    let beta = &data.cores[1];
    assert_eq!(beta.core_uid, 2);
    assert_eq!(beta.report_name, "beta");
    assert_eq!(beta.trades, 1);
    assert_eq!(beta.wins, 0);
    assert!((beta.profit + 5.0).abs() < 1e-9);
    assert!((beta.average_order() - 200.0).abs() < 1e-9);
}

/// `profit_monitor.rs:profit_monitor_on` must return the split scope before constructing scalar
/// rows; replacing that early return with a native projection makes this assertion red and lets
/// the widget display a plausible but false sum of two quote currencies.
#[test]
fn mixed_quote_scope_never_produces_a_combined_monitor_total() {
    let connection = fixture();
    connection
        .execute(
            "UPDATE orders_rep SET basecurrency = 8 WHERE closedate = 110",
            [],
        )
        .expect("mixed-quote fixture update");
    let scoped = profit_monitor_on(&connection, &query(0, 200)).expect("monitor split scope");
    let ProfitScope::Split(totals) = scoped else {
        panic!("mixed quote currencies must never reach scalar monitor rows");
    };
    assert_eq!(totals.orders, 4);
    assert_eq!(totals.totals.len(), 2);
}

/// `profit_monitor.rs:profit_monitor_on` must report the NEWEST trade of each core without
/// disturbing the latest-nonblank-name rule beside it.
///
/// The fixture is built for exactly this collision: core 1's newest row (closedate 130) carries a
/// blank name, so the name must still come from closedate 120 while the last trade comes from 130.
/// Folding the two answers into ONE query would need two MIN/MAX aggregates, and SQLite then picks
/// the bare `core_name` from an arbitrary row — silently, and only on cores that ever traded
/// unnamed. That is why the last trade is read by its own pass.
#[test]
fn latest_trade_is_the_newest_row_even_when_it_is_unnamed() {
    let connection = fixture();
    let scoped = profit_monitor_on(&connection, &query(0, 200)).expect("monitor aggregate");
    let ProfitScope::Comparable { data, .. } = scoped else {
        panic!("single-quote fixture must be comparable");
    };

    let alpha = &data.cores[0];
    assert_eq!(alpha.report_name, "alpha-new");
    assert_eq!(alpha.last_close, 130);
    assert_eq!(alpha.last_profit, Some(-2.0));

    let beta = &data.cores[1];
    assert_eq!(beta.last_close, 110);
    assert_eq!(beta.last_profit, Some(-5.0));
}

/// The last trade must obey the same period bounds as the totals beside it.
///
/// Breakage: dropping the period parameters from the second pass shows a trade from outside the
/// selected hour or day next to a total that excludes it.
#[test]
fn latest_trade_respects_the_selected_period() {
    let connection = fixture();
    let scoped = profit_monitor_on(&connection, &query(0, 125)).expect("monitor aggregate");
    let ProfitScope::Comparable { data, .. } = scoped else {
        panic!("single-quote fixture must be comparable");
    };

    let alpha = &data.cores[0];
    assert_eq!(
        alpha.trades, 2,
        "the fixture's 130 row is outside this period"
    );
    assert_eq!(alpha.last_close, 120);
    assert_eq!(alpha.last_profit, Some(3.0));
}

/// Two trades closing in the SAME Unix second must keep showing ONE amount.
///
/// This pins an assumption rather than a guarantee, deliberately. SQLite says a tie on the guiding
/// min/max lets the bare `pnl` come from any of the tied rows and "the choice is arbitrary" — but
/// arbitrary is not random: for identical data and one plan it returns the same row, which is what
/// the user actually needs, because the monitor re-reads every few seconds and a value that
/// alternated would read as trades that never happened. Buying a written guarantee costs a sort of
/// the whole period (measured: 28 ms → 74 ms on a 50k-row fixture), so this test is the watchman
/// instead. If a future SQLite ever makes that choice vary, this goes red and the ranked form is
/// the answer.
#[test]
fn a_same_second_tie_still_has_one_stable_last_trade() {
    let connection = fixture();
    connection
        .execute(
            "INSERT INTO orders_rep VALUES (1, 'alpha-tie', 'ADA', 0, 95, 130, 7.5, 5, 0, 10.0, 1)",
            [],
        )
        .expect("tied close date");

    let read = || {
        let scoped = profit_monitor_on(&connection, &query(0, 200)).expect("monitor aggregate");
        let ProfitScope::Comparable { data, .. } = scoped else {
            panic!("single-quote fixture must be comparable");
        };
        (data.cores[0].last_close, data.cores[0].last_profit)
    };
    let first = read();
    assert_eq!(first.0, 130, "the newest close date is not in question");
    let tied = [Some(7.5), Some(-2.0)];
    assert!(
        tied.contains(&first.1),
        "the amount must come from one of the tied rows, not from an older trade"
    );
    for _ in 0..4 {
        assert_eq!(
            read(),
            first,
            "the same data must keep giving the same amount"
        );
    }
}

/// A newest trade whose projected profit is NULL must show NO last trade, not a zero.
///
/// The aggregate wraps its `SUM` in `COALESCE` because this NULL exists; reading the picked row's
/// profit with the same `unwrap_or(0.0)` would turn "unknown" into "made nothing" on screen, while
/// the close date is still real enough to drive the arrival highlight.
#[test]
fn a_null_profit_on_the_newest_trade_shows_no_last_trade() {
    let connection = fixture();
    connection
        .execute(
            "INSERT INTO orders_rep VALUES (2, 'beta', 'ADA', 0, 95, 140, NULL, 5, 0, 10.0, 1)",
            [],
        )
        .expect("null-profit row");

    let scoped = profit_monitor_on(&connection, &query(0, 200)).expect("monitor aggregate");
    let ProfitScope::Comparable { data, .. } = scoped else {
        panic!("single-quote fixture must be comparable");
    };
    let beta = &data.cores[1];
    assert_eq!(beta.last_close, 140, "the arrival timestamp is still real");
    assert_eq!(
        beta.last_profit, None,
        "an unknown profit is not a zero one"
    );
}
