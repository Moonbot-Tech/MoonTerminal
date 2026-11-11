//! Report period regressions and scale coverage across historical core-clock offsets.

use std::collections::HashMap;
use std::time::Instant;

use rusqlite::Connection;

use super::super::{ReportFilter, RowScope, query_reports, query_totals};
use super::seeks::{replica, seed};
use crate::db::{OffsetSegment, ReportAxis};

/// Two offset segments matching the survey's transition at true UTC 1500.
fn transition_axis() -> ReportAxis {
    ReportAxis::from_measured(
        HashMap::from([(
            1,
            vec![
                OffsetSegment {
                    from_utc: 0,
                    offset_secs: 0,
                },
                OffsetSegment {
                    from_utc: 1500,
                    offset_secs: 3600,
                },
            ],
        )]),
        chrono_tz::UTC,
    )
}

/// Applying the query-time offset to historical Report rows drops an in-period close and its total.
#[test]
fn correctness_2_report_axis_period_uses_each_rows_segment() {
    let conn = Connection::open_in_memory().expect("in-memory Report fixture");
    conn.execute_batch(
        "CREATE TABLE orders_rep (
            core_uid INTEGER, newrecid INTEGER, closedate INTEGER,
            profitbtc REAL, spentbtc REAL, basecurrency INTEGER
        );
        INSERT INTO orders_rep VALUES
            (1, 1, 1400, 7.0, 100.0, 1),
            (1, 2, 1999, 99.0, 100.0, 1);",
    )
    .expect("seed Report offset transition");
    let filter = ReportFilter {
        core_uids: vec![1],
        date_from: Some(1000),
        date_to: Some(2000),
        rows: RowScope::Closed,
        axis: transition_axis(),
        ..Default::default()
    };

    let table = query_reports(&conn, &filter, "closedate", false, 10).expect("Report rows");
    assert_eq!(table.rec_ids, vec![1]);
    let totals = query_totals(&conn, &filter).expect("Report totals");
    assert_eq!(totals.quotes.orders, 1);
    assert_eq!(totals.quotes.totals[0].profit, 7.0);

    let mut open_basis = filter.clone();
    open_basis.period_basis = super::super::PeriodBasis::OpenDate;
    conn.execute("ALTER TABLE orders_rep ADD COLUMN buydate INTEGER", [])
        .expect("add open-date column");
    conn.execute("UPDATE orders_rep SET buydate = closedate", [])
        .expect("seed open-date values");
    let table =
        query_reports(&conn, &open_basis, "closedate", false, 10).expect("open-basis Report rows");
    assert_eq!(table.rec_ids, vec![1]);
}

/// Sorting the date columns by the raw core-local stamp drops a trade from a core whose clock runs
/// behind the others off the page, although its printed time is the newest (#1058).
#[test]
fn report_date_sort_orders_and_cuts_by_each_cores_corrected_instant() {
    let conn = Connection::open_in_memory().expect("in-memory Report fixture");
    // Core 1 runs three hours behind UTC, core 2 is measured on UTC, core 3 is never measured.
    // In true UTC record 1 opened and closed last; on the raw clocks it is the oldest row. Its
    // sell-set stamp is stored REAL, which the cell still prints as a corrected instant.
    conn.execute_batch(
        "CREATE TABLE orders_rep (
            core_uid INTEGER, newrecid INTEGER, buydate INTEGER, closedate INTEGER,
            sellsetdate INTEGER, profitbtc REAL, spentbtc REAL, basecurrency INTEGER
        );
        INSERT INTO orders_rep VALUES
            (1, 1, 9100, 9200, 9150.5, 1.0, 100.0, 1),
            (2, 2, 9900, 10002, 9950, 1.0, 100.0, 1),
            (2, 3, 9800, 10001, 9850, 1.0, 100.0, 1),
            (3, 4, 9700, 10000, 9750, 1.0, 100.0, 1);",
    )
    .expect("seed Report rows on three clocks");
    let axis = ReportAxis::from_measured(
        HashMap::from([
            (
                1,
                vec![OffsetSegment {
                    from_utc: 0,
                    offset_secs: -10_800,
                }],
            ),
            (
                2,
                vec![OffsetSegment {
                    from_utc: 0,
                    offset_secs: 0,
                }],
            ),
        ]),
        chrono_tz::UTC,
    );
    let all_cores = ReportFilter {
        rows: RowScope::Closed,
        axis,
        ..Default::default()
    };
    let mut listed = all_cores.clone();
    listed.core_uids = vec![1, 2, 3];
    for filter in [&all_cores, &listed] {
        for col in ["buydate", "closedate", "sellsetdate"] {
            let newest = query_reports(&conn, filter, col, true, 2).expect("newest rows");
            assert_eq!(
                newest.rec_ids,
                vec![1, 2],
                "{col} DESC, cores {:?}",
                filter.core_uids
            );
            let oldest = query_reports(&conn, filter, col, false, 2).expect("oldest rows");
            assert_eq!(
                oldest.rec_ids,
                vec![4, 3],
                "{col} ASC, cores {:?}",
                filter.core_uids
            );
        }
    }
}

/// Splitting a date sort by offset segment must still return the rows whose sort column is NULL.
#[test]
fn report_date_sort_across_segments_keeps_rows_without_the_stamp() {
    let conn = Connection::open_in_memory().expect("in-memory Report fixture");
    conn.execute_batch(
        "CREATE TABLE orders_rep (
            core_uid INTEGER, newrecid INTEGER, closedate INTEGER, sellsetdate INTEGER,
            profitbtc REAL, spentbtc REAL, basecurrency INTEGER
        );
        INSERT INTO orders_rep VALUES
            (1, 1, 1400, 1400, 1.0, 100.0, 1),
            (1, 2, 5000, NULL, 1.0, 100.0, 1),
            (1, 3, 5100, 5100, 1.0, 100.0, 1);",
    )
    .expect("seed Report rows across a segment change");
    let filter = ReportFilter {
        rows: RowScope::Closed,
        axis: transition_axis(),
        ..Default::default()
    };
    let table = query_reports(&conn, &filter, "sellsetdate", true, 10).expect("Report rows");
    assert_eq!(table.rec_ids, vec![3, 1, 2]);
}

/// Median milliseconds of six executions of one reader.
fn median_ms(mut read: impl FnMut()) -> f64 {
    let mut samples = (0..6)
        .map(|_| {
            let started = Instant::now();
            read();
            started.elapsed().as_secs_f64() * 1000.0
        })
        .collect::<Vec<_>>();
    samples.sort_by(f64::total_cmp);
    samples[3]
}

/// Measure Report rows and totals over 100000 deals split across historical offset segments.
#[test]
#[ignore = "synthetic reader timing probe"]
fn correctness_2_report_axis_period_scale_probe() {
    let conn = replica();
    seed(&conn, 100_000, 10, 60, 100_001, 8);
    conn.execute("UPDATE orders_rep SET closedate = buydate + 60", [])
        .expect("close every scale deal");
    let axis = ReportAxis::from_measured(
        (1..=10_u64)
            .map(|core| {
                (
                    core,
                    vec![
                        OffsetSegment {
                            from_utc: 0,
                            offset_secs: 0,
                        },
                        OffsetSegment {
                            from_utc: 1_700_050_000,
                            offset_secs: 3600,
                        },
                    ],
                )
            })
            .collect(),
        chrono_tz::UTC,
    );
    let filter = ReportFilter {
        date_from: Some(1),
        date_to: Some(2_000_000_000),
        rows: RowScope::Closed,
        axis,
        ..Default::default()
    };
    let rows_ms = median_ms(|| {
        let table = query_reports(&conn, &filter, "closedate", true, 500).expect("scale rows");
        assert_eq!(table.rows.len(), 500);
        std::hint::black_box(table);
    });
    let totals_ms = median_ms(|| {
        let totals = query_totals(&conn, &filter).expect("scale totals");
        assert_eq!(totals.quotes.orders, 100_000);
        std::hint::black_box(totals);
    });
    println!("report axis 100000 rows: {rows_ms:.3} ms; totals: {totals_ms:.3} ms");
}
