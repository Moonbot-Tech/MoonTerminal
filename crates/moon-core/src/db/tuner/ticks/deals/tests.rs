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
             'Sell Price', 1, 0.0, 0.0, 0.0),
            (14, 7, 42, 'ACE', 160, 170, 160000, 170000, 99.0, 99.0, 1000.0, -0.3, 0,
             'Funding', 1, 0.0, 0.0, 0.0),
            (15, 7, 0, 'BEN', 165, 175, 165000, 175000, 50.0, 51.0, 500.0, 10.0, 0,
             'Manual Sell', 1, 0.0, 0.0, 0.0);",
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
    // The funding row and the no-strategy manual sell carry stamps and are still not deals.
    assert_eq!(read.service, 2, "the service rows");
    assert_eq!(
        read.untunable, 0,
        "the kind gate is applied after the kinds resolve, not here"
    );
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
    // The scan leaves the USDT money to the overlay.
    assert_eq!(ace.profit, None);
}

/// The USDT money of every deal comes off the USDT source in the same snapshot, keyed by the
/// row's `reportuid`: on this pure-USDT replica the source resolves native (already USDT) and
/// the overlay hands each deal its `profitbtc`, sign and all, while `fact_pnl` stays what the
/// metric made it.
#[test]
fn the_usdt_overlay_fills_profit_by_report_uid() {
    let conn = replica();
    let (q, src) = tuner_source_on(&conn, &scope()).expect("source");
    let mut read = read_on(&conn, &q, &src).expect("read");
    let usdt_src = crate::db::tuner::tuner_source_usdt_on(&conn, &q)
        .expect("usdt source")
        .expect("a pure-USDT replica is USDT as it is");
    overlay_usdt_profit(&conn, &q, &usdt_src, &mut read.deals).expect("overlay");
    let ben = &read.deals[0];
    let ace = &read.deals[1];
    assert_eq!((ben.report_uid, ace.report_uid), (12, 11));
    assert_eq!(ben.profit, Some(-10.0));
    assert_eq!(ace.profit, Some(10.0));
    assert!((ace.fact_pnl - 1.0).abs() < 1e-9, "per cent, not money");
    assert!(
        (ace.spent - 1000.0).abs() < 1e-9,
        "the spend stays the scan's"
    );
    // A NULL delta reads as zero, never as a missing row.
    assert_eq!(read.deals[0].deltas.d1h, 0.0);
    assert!(
        read.deals.iter().all(|d| d.kind.is_empty()),
        "kinds are resolved by the caller"
    );
}

/// The entry order's creation and its saved corridor come with the row where the core filed
/// them; a zero, a creation after the fill and a half corridor read as not filed, and a replica
/// without the columns (the fixture above) reads them all as absent.
#[test]
fn the_orders_creation_and_corridor_are_read_where_filed() {
    let conn = Connection::open_in_memory().expect("in-memory database");
    conn.execute_batch(
        "CREATE TABLE orders_rep(
            reportuid INTEGER, core_uid INTEGER, strategyid INTEGER, coin TEXT,
            buydate INTEGER, closedate INTEGER, buydatems INTEGER, closedatems INTEGER,
            buyprice REAL, sellprice REAL, spentbtc REAL, profitbtc REAL, isshort INTEGER,
            sellreason TEXT, basecurrency INTEGER, buysetdatems INTEGER,
            buycorridordown REAL, buycorridorup REAL
         );
         INSERT INTO orders_rep VALUES
            (21, 7, 42, 'ACE', 100, 200, 100000, 200000, 99.0, 100.0, 1000.0, 10.0, 0,
             'Sell Price', 1, 95000, 99.6, 98.1),
            (22, 7, 42, 'ACE', 110, 210, 110000, 210000, 99.0, 100.0, 1000.0, 10.0, 0,
             'Sell Price', 1, 0, 0.0, 0.0),
            (23, 7, 42, 'ACE', 120, 220, 120000, 220000, 99.0, 100.0, 1000.0, 10.0, 0,
             'Sell Price', 1, 120001, 99.6, 0.0);",
    )
    .expect("fixture");
    let (q, src) = tuner_source_on(&conn, &scope()).expect("source");
    let read = read_on(&conn, &q, &src).expect("read");
    let by_uid = |uid| {
        read.deals
            .iter()
            .find(|d| d.report_uid == uid)
            .expect("deal")
    };
    assert_eq!(by_uid(21).buy_set_ms, Some(95_000));
    assert_eq!(by_uid(21).corridor, Some((99.6, 98.1)));
    assert_eq!((by_uid(22).buy_set_ms, by_uid(22).corridor), (None, None));
    assert_eq!((by_uid(23).buy_set_ms, by_uid(23).corridor), (None, None));
    let old_conn = replica();
    let (old_q, old_src) = tuner_source_on(&old_conn, &scope()).expect("source");
    let old = read_on(&old_conn, &old_q, &old_src).expect("read");
    assert!(
        old.deals
            .iter()
            .all(|d| d.buy_set_ms.is_none() && d.corridor.is_none())
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
