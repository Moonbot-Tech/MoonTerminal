//! Deterministic ascending Report selection under equal sort keys and per-source limits.

use std::time::Instant;

use rusqlite::{Connection, params};

use super::super::{ReportFilter, RowScope, query_reports};
use super::seeks::{replica, seed};

/// Omitting the replica key before an ascending LIMIT lets planner order choose the retained rows.
#[test]
fn correctness_4_ascending_report_tie_uses_total_replica_key() {
    let conn = Connection::open_in_memory().expect("open ascending tie fixture");
    super::super::super::init_db(&conn).expect("initialize report metadata");
    conn.execute_batch(
        "CREATE TABLE orders_rep (
             core_uid INTEGER NOT NULL, core_name TEXT NOT NULL, newrecid INTEGER NOT NULL,
             closedate INTEGER, PRIMARY KEY (core_uid, newrecid)
         );",
    )
    .expect("create ascending tie replica");
    let seeded = vec![
        (3_i64, 32_i64),
        (1, 20),
        (4, 53),
        (2, 11),
        (1, 50),
        (3, 12),
        (2, 51),
        (4, 23),
        (1, 30),
        (3, 52),
        (2, 21),
        (4, 43),
        (1, 10),
        (3, 42),
        (2, 41),
        (4, 13),
        (1, 40),
        (3, 22),
        (2, 31),
        (4, 33),
    ];
    let mut insert = conn
        .prepare("INSERT INTO orders_rep VALUES (?1, ?2, ?3, 1000)")
        .expect("prepare ascending tie rows");
    for &(core_uid, newrecid) in &seeded {
        insert
            .execute(params![core_uid, format!("CORE-{core_uid}"), newrecid])
            .expect("insert ascending tie row");
    }
    drop(insert);
    super::super::super::test_support::rep_init(&conn);

    let table = query_reports(
        &conn,
        &ReportFilter {
            rows: RowScope::Closed,
            ..Default::default()
        },
        "closedate",
        false,
        9,
    )
    .expect("read ascending tied Report");
    let mut expected = seeded;
    expected.sort_unstable();
    let expected = expected
        .into_iter()
        .take(9)
        .map(|(_, newrecid)| newrecid)
        .collect::<Vec<_>>();
    assert_eq!(table.rec_ids, expected);
}

/// Measure ascending tied selection over 100000 deals without weakening the result-size assertion.
#[test]
#[ignore = "synthetic reader timing probe"]
fn correctness_4_ascending_report_tie_scale_probe() {
    let conn = replica();
    seed(&conn, 100_000, 10, 60, 100_001, 8);
    conn.execute_batch("UPDATE orders_rep SET closedate=1700000000; ANALYZE;")
        .expect("tie every scale row");
    let filter = ReportFilter {
        rows: RowScope::Closed,
        ..Default::default()
    };
    let mut samples = Vec::new();
    for _ in 0..6 {
        let started = Instant::now();
        let table = query_reports(&conn, &filter, "closedate", false, 500)
            .expect("ascending tied scale read");
        assert_eq!(table.rows.len(), 500);
        std::hint::black_box(table);
        samples.push(started.elapsed().as_secs_f64() * 1000.0);
    }
    samples.sort_by(f64::total_cmp);
    println!("report ascending tied top-N 100000: {:.3} ms", samples[3]);
}
