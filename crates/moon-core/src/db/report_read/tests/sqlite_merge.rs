//! SQLite-compatible value ordering in the Rust merge of typed and legacy Report sources.

use std::cmp::Ordering;
use std::time::Instant;

use rusqlite::Connection;
use rusqlite::types::Value;

use super::super::{ReportFilter, RowScope, cmp_values, query_reports};

/// Integer precision and SQLite's NULL < numbers < text < blob classes must survive the merge.
#[test]
fn correctness_5_sqlite_merge_order_is_exact() {
    assert_eq!(
        cmp_values(
            &Value::Integer(9_007_199_254_740_993),
            &Value::Integer(9_007_199_254_740_992),
        ),
        Ordering::Greater
    );
    assert_eq!(
        cmp_values(
            &Value::Integer(9_007_199_254_740_993),
            &Value::Real(9_007_199_254_740_992.0),
        ),
        Ordering::Greater
    );
    assert_eq!(
        cmp_values(&Value::Integer(2), &Value::Text("10".into())),
        Ordering::Less
    );
    assert_eq!(
        cmp_values(&Value::Text("10".into()), &Value::Blob(vec![49])),
        Ordering::Less
    );
    assert_eq!(cmp_values(&Value::Null, &Value::Integer(2)), Ordering::Less);
}

/// Measure the merged typed/legacy top-N over 100000 rows with mixed storage classes.
#[test]
#[ignore = "synthetic reader timing probe"]
fn correctness_5_sqlite_merge_scale_probe() {
    let conn = Connection::open_in_memory().expect("in-memory mixed-source scale report");
    conn.execute_batch(
        "CREATE TABLE orders_rep (
            core_uid INTEGER, core_name TEXT, newrecid INTEGER, closedate INTEGER, sort_key,
            PRIMARY KEY (core_uid, newrecid)
        );
        CREATE TABLE closed_sell_reports (
            core_uid INTEGER, core_name TEXT, db_id INTEGER, closedate INTEGER, sort_key,
            PRIMARY KEY (core_uid, db_id)
        );
        WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<50000)
        INSERT INTO orders_rep
            SELECT x%10+1, 'CORE_A', x, 1700000000,
                CASE x%4 WHEN 0 THEN 9007199254740992+x%2 WHEN 1 THEN 2.0
                 WHEN 2 THEN '10' ELSE X'31' END FROM n;
        WITH RECURSIVE n(x) AS (VALUES(50001) UNION ALL SELECT x+1 FROM n WHERE x<100000)
        INSERT INTO closed_sell_reports
            SELECT x%10+1, 'CORE_B', x, 1700000000,
                CASE x%4 WHEN 0 THEN 9007199254740992+x%2 WHEN 1 THEN 2.0
                 WHEN 2 THEN '10' ELSE X'31' END FROM n;
        ANALYZE;",
    )
    .expect("seed mixed-source scale report");
    let filter = ReportFilter {
        rows: RowScope::Closed,
        ..Default::default()
    };
    let mut samples = Vec::new();
    for _ in 0..6 {
        let started = Instant::now();
        let table = query_reports(&conn, &filter, "sort_key", false, 500)
            .expect("mixed storage-class scale read");
        assert_eq!(table.rows.len(), 500);
        std::hint::black_box(table);
        samples.push(started.elapsed().as_secs_f64() * 1000.0);
    }
    samples.sort_by(f64::total_cmp);
    println!("report SQLite merge top-N 100000: {:.3} ms", samples[3]);
}
