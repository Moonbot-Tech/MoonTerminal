//! Per-core spend-basis detection.
//!
//! The cached [`super::margin_cores`] wrapper is deliberately untested: it is a TTL around
//! [`super::probe`], and a process-wide cache cannot be exercised from tests that run in
//! parallel without them fighting over the same slot. Everything the verdict depends on lives
//! in the probe.

use super::*;
use rusqlite::Connection;

/// In-memory replica carrying every column the probe needs.
///
/// Args:
///     rows: `(core_uid, boughtq, buyprice, lev, spentbtc, sellreason)` fixtures.
///
/// Returns:
///     Seeded report connection.
fn seed(rows: &[(i64, f64, f64, i64, f64, &str)]) -> Connection {
    let c = Connection::open_in_memory().unwrap();
    c.execute_batch(
        "CREATE TABLE orders_rep(
            core_uid INTEGER, closedate INTEGER, boughtq REAL, buyprice REAL,
            lev INTEGER, spentbtc REAL, sellreason TEXT, deleted INTEGER
         );",
    )
    .unwrap();
    for (uid, bq, bp, lev, spent, reason) in rows {
        c.execute(
            "INSERT INTO orders_rep(
                core_uid, closedate, boughtq, buyprice, lev, spentbtc, sellreason, deleted
             ) VALUES (?1, 1700000000, ?2, ?3, ?4, ?5, ?6, 0)",
            rusqlite::params![uid, bq, bp, lev, spent, reason],
        )
        .unwrap();
    }
    c
}

/// One margin row: the core posted `notional / leverage`, plus the fee already baked in.
fn margin_row(uid: i64, lev: i64) -> (i64, f64, f64, i64, f64, &'static str) {
    let (bq, bp) = (100.0, 2.0);
    (uid, bq, bp, lev, bq * bp / lev as f64 * 1.0002, "Sell Price")
}

/// One notional row: the core posted the whole traded value.
fn notional_row(uid: i64, lev: i64) -> (i64, f64, f64, i64, f64, &'static str) {
    let (bq, bp) = (100.0, 2.0);
    (uid, bq, bp, lev, bq * bp * 1.0002, "Sell Price")
}

/// A core whose spend column reconstructs the notional only after multiplying by the row's
/// leverage must be flagged, or its turnover is understated by that factor.
#[test]
fn margin_core_is_detected_across_mixed_leverage() {
    // One core, four different leverages — the multiplier is per-row, not per-core.
    let rows: Vec<_> = [5, 10, 25, 100]
        .iter()
        .map(|lev| margin_row(7, *lev))
        .collect();
    let cores = probe(&seed(&rows)).unwrap();
    assert_eq!(cores, HashSet::from([7]));
}

/// The common convention must stay unflagged: multiplying a notional core by leverage would
/// inflate its volume by up to 100×.
#[test]
fn notional_core_is_not_flagged() {
    let rows: Vec<_> = [5, 20].iter().map(|lev| notional_row(3, *lev)).collect();
    assert!(probe(&seed(&rows)).unwrap().is_empty());
}

/// An inverse COIN-M core matches NEITHER candidate, because `boughtq` counts contracts. A few
/// coincidental hits must not elect it margin — the live replica had 11 such hits in 6 637 rows.
#[test]
fn inverse_core_with_coincidental_hits_defaults_to_notional() {
    let mut rows = vec![
        // Contracts against a coin-denominated spend: neither candidate reconstructs it.
        (9, 2537.0, 128_146.0, 15, 2.05, "Sell Price"),
        (9, 14374.0, 3629.0, 20, 1.28, "Sell Price"),
    ];
    // Two rows that happen to look like margin, still a minority of the core's four.
    rows.push(margin_row(9, 10));
    rows.push(margin_row(9, 10));
    assert!(probe(&seed(&rows)).unwrap().is_empty());
}

/// A bare majority is what decides it, so the tie must fall to the safe default.
#[test]
fn an_even_split_stays_notional() {
    let rows = vec![margin_row(4, 10), notional_row(4, 10)];
    assert!(probe(&seed(&rows)).unwrap().is_empty());
}

/// Funding rows carry `spentbtc = boughtq` and match neither convention; counting them would
/// dilute the majority a real margin core needs.
#[test]
fn funding_rows_do_not_dilute_the_verdict() {
    let mut rows = vec![margin_row(2, 10), margin_row(2, 25)];
    // Three funding pseudo-orders on the same core, each with spend == quantity.
    for _ in 0..3 {
        rows.push((2, 350.0, 0.0013, 10, 350.0, "Funding"));
    }
    assert_eq!(probe(&seed(&rows)).unwrap(), HashSet::from([2]));
}

/// A source missing any probe input cannot be classified, and must not error the read.
#[test]
fn source_without_the_inputs_is_skipped() {
    let c = Connection::open_in_memory().unwrap();
    c.execute_batch("CREATE TABLE orders_rep(core_uid INTEGER, closedate INTEGER);")
        .unwrap();
    assert!(probe(&c).unwrap().is_empty());
}

/// Without margin cores the factor must collapse to a literal, keeping the common SQL free of a
/// per-row CASE.
#[test]
fn factor_is_one_without_margin_cores() {
    assert_eq!(notional_factor_expr("o", &HashSet::new()), "1");
}

/// The factor names the row's own leverage and floors it at 1 — the replica holds a `lev = 0` row,
/// and a zero multiplier would erase that trade from the turnover.
#[test]
fn factor_uses_row_leverage_and_floors_it() {
    let sql = notional_factor_expr("o", &HashSet::from([5]));
    assert!(sql.contains("o.core_uid IN (5)"), "{sql}");
    assert!(sql.contains("MAX(COALESCE(o.lev, 1), 1)"), "{sql}");
}
