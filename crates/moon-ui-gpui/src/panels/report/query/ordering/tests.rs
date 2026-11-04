//! Regression coverage for the reserved open block and its parallel row identities.

use super::{Ordering, ReportTable, RowScope, Value, compare_cells, sort_open_prefix};

/// Build a small already-reserved page with closed rows deliberately outside the open sort range.
fn page() -> ReportTable {
    ReportTable {
        cols: ["closedate", "buydate", "valuation_profit_usdt"]
            .map(String::from)
            .to_vec(),
        rows: vec![
            vec![Value::Integer(0), Value::Integer(300), Value::Real(-2.0)],
            vec![Value::Null, Value::Integer(200), Value::Integer(9)],
            vec![
                Value::Text("invalid".into()),
                Value::Integer(100),
                Value::Null,
            ],
            vec![Value::Integer(400), Value::Integer(50), Value::Real(-100.0)],
            vec![Value::Integer(500), Value::Integer(400), Value::Real(100.0)],
        ],
        core_uids: vec![1, 2, 3, 4, 5],
        rec_ids: vec![11, 12, 13, 14, 15],
    }
}

/// Removing the UI permutation or reversing its direction leaves the open block ignoring arrows;
/// sorting the whole page instead moves closed trades into the reserved leading block.
#[test]
fn open_prefix_follows_numeric_and_time_headers_in_both_directions() {
    for (key, desc, expected) in [
        ("valuation_profit_usdt", false, vec![11, 12, 13, 14, 15]),
        ("valuation_profit_usdt", true, vec![12, 11, 13, 14, 15]),
        ("buydate", false, vec![13, 12, 11, 14, 15]),
        ("buydate", true, vec![11, 12, 13, 14, 15]),
    ] {
        let mut table = page();
        let original = table.rows.clone();
        sort_open_prefix(&mut table, RowScope::ClosedAndOpenIfCurrent, key, desc);
        assert_eq!(table.rec_ids, expected, "{key}, desc={desc}");
        for (index, rec) in table.rec_ids.iter().enumerate() {
            let source = (*rec - 11) as usize;
            assert_eq!(table.core_uids[index], (source + 1) as u64);
            assert_eq!(table.rows[index], original[source]);
        }
    }
}

/// A closed-only request must keep database order, even for a legacy row with no close time.
#[test]
fn closed_only_and_unavailable_headers_preserve_the_page() {
    for (scope, key) in [
        (RowScope::Closed, "buydate"),
        (RowScope::ClosedAndOpen, "absent"),
    ] {
        let mut table = page();
        sort_open_prefix(&mut table, scope, key, false);
        assert_eq!(table.rec_ids, vec![11, 12, 13, 14, 15]);
    }
}

/// Casting integer identifiers to floats would collapse adjacent large ids into a false tie.
#[test]
fn numeric_comparison_keeps_integer_precision_and_sqlite_type_order() {
    let values = [
        Value::Integer(i64::MIN),
        Value::Real(-0.5),
        Value::Integer(0),
        Value::Integer(9_007_199_254_740_992),
        Value::Integer(9_007_199_254_740_993),
        Value::Real(9_223_372_036_854_775_808.0),
        Value::Text("A".into()),
        Value::Blob(vec![0]),
        Value::Null,
    ];
    for pair in values.windows(2) {
        assert_eq!(compare_cells(&pair[0], &pair[1], false), Ordering::Less);
        let expected = if pair[1] == Value::Null {
            Ordering::Less
        } else {
            Ordering::Greater
        };
        assert_eq!(compare_cells(&pair[0], &pair[1], true), expected);
    }
}

/// Sorting must spend no extra slots or drop positions when the reserved block fills the page.
/// The 501-row fixture is the smallest that crosses the actual 500-row database reservation.
#[test]
fn database_reservation_survives_display_sort_at_the_page_limit() {
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    conn.execute_batch(
        "CREATE TABLE orders_rep (
            core_uid INTEGER, newrecid INTEGER, core_name TEXT, buydate INTEGER,
            closedate INTEGER, profitbtc REAL, PRIMARY KEY(core_uid, newrecid)
         );
         WITH RECURSIVE rows(n) AS (VALUES(1) UNION ALL SELECT n+1 FROM rows WHERE n<501)
         INSERT INTO orders_rep SELECT 1, n, 'CORE_A', n, 0, n FROM rows;
         INSERT INTO orders_rep VALUES (1, 502, 'CORE_A', 0, 600, -1000);",
    )
    .unwrap();
    let mut table = moon_core::db::query_reports(
        &conn,
        &Default::default(),
        "buydate",
        false,
        super::super::MAX_REPORT_ROWS,
    )
    .unwrap();
    let mut reserved = table.rec_ids.clone();
    reserved.sort_unstable();
    sort_open_prefix(&mut table, RowScope::ClosedAndOpen, "buydate", false);
    assert_eq!(table.rec_ids, (2..=501).collect::<Vec<_>>());
    assert_eq!(table.rec_ids, reserved);
    sort_open_prefix(&mut table, RowScope::ClosedAndOpen, "buydate", true);
    assert_eq!(table.rec_ids, (2..=501).rev().collect::<Vec<_>>());
}
