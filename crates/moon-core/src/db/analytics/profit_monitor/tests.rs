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
