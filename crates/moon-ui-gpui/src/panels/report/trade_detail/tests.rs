//! Fixture coverage for Report-period neighbours and the retained drawing selection.

use super::period_history;
use moon_core::db::{OffsetSegment, ReportAxis, ReportFilter, ReportStrategyKey, SideFilter};
use rusqlite::Connection;

/// Create closed-trade fixtures with real Report column names and no live application state.
fn fixture() -> Connection {
    let conn = Connection::open_in_memory().expect("fixture DB");
    conn.execute_batch(
        "CREATE TABLE orders_rep (
            core_uid INTEGER, newrecid INTEGER, coin TEXT, buydate INTEGER, closedate INTEGER,
            buyprice REAL, sellprice REAL, quantity REAL, isshort INTEGER,
            emulator INTEGER, deleted INTEGER, strategyid INTEGER
        );
        INSERT INTO orders_rep VALUES
            (7, 1, 'BTC', 50, 100, 10, 12, 2, 0, 0, 0, 4),
            (7, 2, 'BTC', 200, 250, 10, 12, 2, 0, 0, 0, 4),
            (7, 3, 'BTC', 50, 250, 10, 12, 2, 0, 0, 0, 4),
            (7, 4, 'BTC', 201, 250, 10, 12, 2, 0, 0, 0, 4),
            (7, 5, 'BTC', 50, 99, 10, 12, 2, 0, 0, 0, 4),
            (8, 6, 'BTC', 110, 120, 10, 12, 2, 0, 0, 0, 4),
            (7, 7, 'BTC-PERP', 110, 120, 10, 12, 2, 0, 0, 0, 4),
            (7, 8, 'BTC', 110, 120, 10, 12, 2, 1, 0, 0, 4),
            (7, 9, 'BTC', 110, 120, 10, 12, 2, 0, 1, 0, 4),
            (7, 10, 'BTC', 110, 120, 10, 12, 2, 0, 0, 1, 4),
            (7, 11, 'BTC', 110, 120, 10, 12, 2, 0, 0, 0, 5),
            (7, 12, 'BTC', 110, 0, 10, 12, 2, 0, 0, 0, 4);",
    )
    .expect("seed neighbours");
    conn
}

/// Close-only selection loses id 2; widening scope admits independently seeded decoys.
#[test]
fn neighbours_respect_either_endpoint_and_report_predicates() {
    let conn = fixture();
    let filter = ReportFilter {
        date_from: Some(100),
        date_to: Some(200),
        side: SideFilter::Long,
        emulator: Some(false),
        strategies: Some(vec![ReportStrategyKey {
            core_uid: 7,
            strategy_id: 4,
        }]),
        ..ReportFilter::default()
    };
    let rows = period_history(&conn, 7, "btc", &filter).expect("period history");
    assert_eq!(
        rows.iter().map(|r| r.record_id).collect::<Vec<_>>(),
        vec![2, 1]
    );
}

/// Restoring the old 1,000-record cap drops the oldest clicked trade and period neighbours.
#[test]
fn neighbours_include_more_than_the_old_history_cap() {
    let conn = fixture();
    conn.execute_batch(
        "DELETE FROM orders_rep;
        WITH RECURSIVE n(x) AS (SELECT 1 UNION ALL SELECT x + 1 FROM n WHERE x < 1001)
        INSERT INTO orders_rep SELECT 7, x, 'BTC', 100, 200 + x, 10, 12, 2, 0, 0, 0, 4 FROM n;",
    )
    .expect("seed more than old cap");
    let filter = ReportFilter {
        date_from: Some(100),
        date_to: Some(100),
        ..ReportFilter::default()
    };
    let rows = period_history(&conn, 7, "BTC", &filter).expect("all neighbours");
    assert_eq!(rows.len(), 1001);
    assert_eq!(rows.last().map(|r| r.record_id), Some(1));
}

/// Comparing raw stamps against UTC bounds loses boundary trades on an offset core.
#[test]
fn neighbours_use_the_report_axis_for_inclusive_bounds() {
    let conn = fixture();
    let axis = ReportAxis::from_measured(
        std::collections::HashMap::from([(
            7,
            vec![OffsetSegment {
                from_utc: 0,
                offset_secs: 3600,
            }],
        )]),
        chrono_tz::UTC,
    );
    let filter = ReportFilter {
        date_from: Some(-3500),
        date_to: Some(-3400),
        axis,
        side: SideFilter::Long,
        emulator: Some(false),
        strategies: Some(vec![ReportStrategyKey {
            core_uid: 7,
            strategy_id: 4,
        }]),
        ..ReportFilter::default()
    };
    let rows = period_history(&conn, 7, "BTC", &filter).expect("offset period");
    assert_eq!(
        rows.iter().map(|r| r.record_id).collect::<Vec<_>>(),
        vec![2, 1]
    );
}
