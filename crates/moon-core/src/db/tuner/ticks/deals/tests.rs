//! The deal read on an in-memory replica: the millisecond gate, the deltas, the order.

use rusqlite::Connection;

use super::*;
use crate::db::tuner::tuner_source_on;

fn replica() -> Connection {
    let conn = Connection::open_in_memory().expect("in-memory database");
    conn.execute_batch(
        "CREATE TABLE orders_rep(
            reportuid INTEGER, core_uid INTEGER, strategyid INTEGER, coin TEXT,
            buydate INTEGER, closedate INTEGER, buydatems INTEGER, closedatems INTEGER,
            buyprice REAL, sellprice REAL, spentbtc REAL, profitbtc REAL, isshort INTEGER,
            sellreason TEXT, basecurrency INTEGER, d1h REAL, d3h REAL, pricebug REAL
         );
         INSERT INTO orders_rep VALUES
            (11, 7, 42, 'ACE', 100, 200, 100000, 200000, 99.0, 100.0, 1000.0, 10.0, 0,
             'Sell Price', 1, 4.2, 6.8, 0.5),
            (12, 7, 42, 'BEN', 150, 180, 150000, 180000, 50.0, 49.0, 500.0, -10.0, 1,
             'Auto Price Down', 1, NULL, -1.0, 0.0),
            (13, 7, 42, 'OLD', 110, 190, 0, 0, 1.0, 1.1, 100.0, 10.0, 0,
             'Sell Price', 1, 0.0, 0.0, 0.0);",
    )
    .expect("fixture");
    conn
}

fn scope() -> Query {
    Query {
        from: 1,
        to: 1_000,
        metric: crate::db::ProfitMetric::Percent,
        ..Default::default()
    }
}

#[test]
fn rows_with_stamps_become_deals_and_the_rest_are_counted() {
    let conn = replica();
    let (q, src) = tuner_source_on(&conn, &scope()).expect("source");
    let read = read_on(&conn, &q, &src).expect("read");
    assert_eq!(read.without_ms, 1, "the row without millisecond stamps");
    assert_eq!(read.deals.len(), 2);
    // Chronological by the millisecond close: BEN (180 000) before ACE (200 000).
    assert_eq!(read.deals[0].report_uid, 12);
    assert_eq!(read.deals[1].report_uid, 11);
    let ace = &read.deals[1];
    assert_eq!(ace.coin, "ACE");
    assert_eq!((ace.buy_ms, ace.close_ms), (100_000, 200_000));
    assert!(!ace.is_short && read.deals[0].is_short);
    assert_eq!(ace.sell_reason, "Sell Price");
    assert!((ace.deltas.d1h - 4.2).abs() < 1e-9);
    assert!((ace.deltas.d3h - 6.8).abs() < 1e-9);
    assert!((ace.deltas.pricebug - 0.5).abs() < 1e-9);
    // Percent metric: 10 / 1000 · 100.
    assert!((ace.fact_pnl - 1.0).abs() < 1e-9, "{}", ace.fact_pnl);
    // A NULL delta reads as zero, never as a missing row.
    assert_eq!(read.deals[0].deltas.d1h, 0.0);
    assert!(
        read.deals.iter().all(|d| d.kind.is_empty()),
        "kinds are resolved by the caller"
    );
}

#[test]
fn a_replica_without_the_stamp_columns_yields_no_deals() {
    let conn = Connection::open_in_memory().expect("in-memory database");
    conn.execute_batch(
        "CREATE TABLE orders_rep(
            closedate INTEGER, core_uid INTEGER, profitbtc REAL, spentbtc REAL,
            basecurrency INTEGER
         );
         INSERT INTO orders_rep VALUES (100, 1, 10.0, 100.0, 1);",
    )
    .expect("fixture");
    let (q, src) = tuner_source_on(&conn, &scope()).expect("source");
    let read = read_on(&conn, &q, &src).expect("read");
    assert!(read.deals.is_empty());
    assert_eq!(read.without_ms, 1);
}
