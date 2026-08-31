//! Summary current-stream statement-count regression tests.

use std::collections::HashMap;
use std::sync::{Mutex, MutexGuard};

use rusqlite::trace::{TraceEvent, TraceEventCodes};

use super::*;

/// Serializes the process-global function-pointer trace sink used by rusqlite.
static TRACE_GUARD: Mutex<()> = Mutex::new(());
/// SQL statements executed while the local test connection has tracing enabled.
static TRACE_SQL: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// `summary_stream.rs:Accumulator::finish_period` must reject a grid that would exceed its
/// span-derived cap; removing that cap lets an out-of-period observed bucket silently expand the
/// Summary grid instead of returning the classified failure that prevents an allocation abort.
#[test]
fn finish_period_rejects_a_distant_bucket_after_the_span_cap() {
    const DAY: i64 = 86_400;
    let mut accumulator = Accumulator {
        days: vec![
            DayPoint {
                start: 0,
                profit: 1.0,
                trades: 1,
            },
            DayPoint {
                start: 3 * DAY,
                profit: 2.0,
                trades: 1,
            },
        ],
        ..Default::default()
    };

    let error = accumulator
        .finish_period(DAY, chrono_tz::UTC, 2)
        .expect_err("two span slots cannot consume a bucket three days away");

    assert!(
        matches!(
            error,
            rusqlite::Error::InvalidColumnType(_, message, rusqlite::types::Type::Null)
                if message == "summary grid exceeded max_buckets"
        ),
        "the bounded grid must surface its classified overflow"
    );
}

/// Record one statement reported by SQLite's execution trace.
fn record_sql(event: TraceEvent<'_>) {
    if let TraceEvent::Stmt(statement, _) = event {
        TRACE_SQL
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push(
                statement
                    .expanded_sql()
                    .unwrap_or_else(|| statement.sql().into_owned()),
            );
    }
}

/// Acquire the trace sink and clear any statements from an earlier failed test.
fn start_trace() -> MutexGuard<'static, ()> {
    let guard = TRACE_GUARD
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    TRACE_SQL
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clear();
    guard
}

/// `summary_stream::read` -- collapsing a long period back into one execution lets SQLite sort
/// the whole current-period result, while shrinking or overlapping a window changes the traced
/// half-open bounds and can duplicate or omit rows.
#[test]
fn current_summary_rows_execute_multiple_bounded_statements() {
    let _trace = start_trace();
    let conn = Connection::open_in_memory().expect("in-memory database");
    conn.execute_batch(
        "CREATE TABLE current_rows(
            closedate INTEGER, buydate INTEGER, pnl REAL, core_uid INTEGER,
            core_name TEXT, coin TEXT, strategyid INTEGER, isshort INTEGER,
            profitbtc REAL, spentbtc REAL, basecurrency INTEGER
         );
         INSERT INTO current_rows VALUES
            (100, 90, 2.0, 1, 'alpha', 'BTC', 7, 0, 2.0, 20.0, 1),
            (200, 180, -1.0, 1, 'alpha', 'ETH', 7, 1, -1.0, 20.0, 1);",
    )
    .expect("summary trace fixture");
    let query = Query {
        from: 1,
        to: 2 * QUERY_WINDOW_SECS + 2,
        ..Default::default()
    };
    let source = "(SELECT * FROM current_rows WHERE closedate >= ?1 AND closedate < ?2) o";

    conn.trace_v2(TraceEventCodes::SQLITE_TRACE_STMT, Some(record_sql));
    let result = read(&conn, source, None, &query, &query.axis, 3_600, true, false);
    conn.trace_v2(TraceEventCodes::empty(), None);
    result.expect("bounded current-period stream");

    let statements = TRACE_SQL
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone();
    assert_eq!(
        statements.len(),
        3,
        "current Summary statements: {statements:#?}"
    );
    for (index, statement) in statements.iter().enumerate() {
        let from = 1 + index as i64 * QUERY_WINDOW_SECS;
        let to = (from + QUERY_WINDOW_SECS).min(query.to);
        assert!(statement.contains(&format!(
            "FROM (SELECT * FROM current_rows WHERE closedate >= {from} AND closedate < {to}) o"
        )));
        assert!(
            statement.contains(&format!(
                "WHERE o.closedate >= {from} AND o.closedate < {to}"
            )),
            "statement {index} must carry its own bounded half-open interval: {statement}"
        );
    }
}

/// `summary_stream::read` -- an empty interval executes no row statement, a short interval uses
/// one, and exact seven-day boundaries assign a row to exactly one adjacent execution.
#[test]
fn current_summary_windows_cover_empty_short_exact_and_multi_periods_once() {
    let conn = Connection::open_in_memory().expect("in-memory database");
    conn.execute_batch(&format!(
        "CREATE TABLE current_rows(
            closedate INTEGER, buydate INTEGER, pnl REAL, core_uid INTEGER,
            core_name TEXT, coin TEXT, strategyid INTEGER, isshort INTEGER,
            profitbtc REAL, spentbtc REAL, basecurrency INTEGER
         );
         INSERT INTO current_rows VALUES
            (100, 90, 1.0, 1, 'alpha', 'A', 7, 0, 1.0, 10.0, 1),
            ({}, 90, 2.0, 1, 'alpha', 'B', 7, 0, 2.0, 10.0, 1),
            ({}, 90, 4.0, 1, 'alpha', 'C', 7, 0, 4.0, 10.0, 1),
            ({}, 90, 8.0, 1, 'alpha', 'D', 7, 0, 8.0, 10.0, 1);",
        100 + QUERY_WINDOW_SECS - 1,
        100 + QUERY_WINDOW_SECS,
        100 + 2 * QUERY_WINDOW_SECS - 1,
    ))
    .expect("window-boundary fixture");
    let source = "(SELECT * FROM current_rows WHERE ?1 <= ?2) o";
    let cases = [
        (100, 100, 0, 0.0),
        (100, 101, 1, 1.0),
        (100, 100 + QUERY_WINDOW_SECS, 2, 3.0),
        (100, 100 + 2 * QUERY_WINDOW_SECS, 4, 15.0),
    ];

    for (from, to, expected_rows, expected_profit) in cases {
        let query = Query {
            from,
            to,
            ..Default::default()
        };
        let result = read(
            &conn,
            source,
            None,
            &query,
            &query.axis,
            86_400,
            false,
            false,
        )
        .expect("bounded window fixture");
        assert_eq!(result.cur.n, expected_rows, "period [{from}, {to})");
        assert_eq!(result.cur.profit, expected_profit, "period [{from}, {to})");
    }
}

/// `read_with_window` -- crossing a production window boundary must produce the same sequence
/// metrics, bucket order, and group totals as one statement over the identical fixture.
#[test]
fn multi_window_results_match_one_window_reference_across_boundary() {
    let conn = Connection::open_in_memory().expect("in-memory database");
    let from = 10_000;
    conn.execute_batch(&format!(
        "CREATE TABLE current_rows(
            closedate INTEGER, buydate INTEGER, pnl REAL, core_uid INTEGER,
            core_name TEXT, coin TEXT, strategyid INTEGER, isshort INTEGER,
            profitbtc REAL, spentbtc REAL, basecurrency INTEGER
         );
         INSERT INTO current_rows VALUES
            ({}, {}, 5.0, 1, 'alpha', 'BTC', 7, 0, 5.0, 20.0, 1),
            ({}, {}, -2.0, 1, 'alpha', 'ETH', 7, 1, -2.0, 20.0, 1),
            ({}, {}, 4.0, 1, 'alpha', 'BTC', 7, 0, 4.0, 20.0, 1);",
        from + QUERY_WINDOW_SECS - 1,
        from + QUERY_WINDOW_SECS - 61,
        from + QUERY_WINDOW_SECS,
        from + QUERY_WINDOW_SECS - 60,
        from + QUERY_WINDOW_SECS + 1,
        from + QUERY_WINDOW_SECS - 59,
    ))
    .expect("cross-boundary fixture");
    let query = Query {
        from,
        to: from + 2 * QUERY_WINDOW_SECS,
        ..Default::default()
    };
    let source = "(SELECT * FROM current_rows WHERE ?1 <= ?2) o";

    let bounded = read_with_window(
        &conn,
        source,
        None,
        &query,
        &query.axis,
        StreamReadConfig {
            bucket: 86_400,
            include_kinds: false,
            has_names: false,
            query_window_secs: QUERY_WINDOW_SECS,
        },
    )
    .expect("multi-window result");
    let reference = read_with_window(
        &conn,
        source,
        None,
        &query,
        &query.axis,
        StreamReadConfig {
            bucket: 86_400,
            include_kinds: false,
            has_names: false,
            query_window_secs: 3 * QUERY_WINDOW_SECS,
        },
    )
    .expect("one-window reference");

    assert_eq!(bounded.cur.n, reference.cur.n);
    assert_eq!(bounded.cur.profit, reference.cur.profit);
    assert_eq!(bounded.cur.max_dd, reference.cur.max_dd);
    assert_eq!(bounded.cur.win_streak, reference.cur.win_streak);
    assert_eq!(bounded.cur.loss_streak, reference.cur.loss_streak);
    assert_eq!(
        bounded
            .days
            .iter()
            .map(|day| (day.start, day.profit, day.trades))
            .collect::<Vec<_>>(),
        reference
            .days
            .iter()
            .map(|day| (day.start, day.profit, day.trades))
            .collect::<Vec<_>>()
    );
    assert_eq!(bounded.strategies[0].n, reference.strategies[0].n);
    assert_eq!(bounded.strategies[0].profit, reference.strategies[0].profit);
    assert_eq!(bounded.coins.len(), reference.coins.len());
}

/// `summary_stream::read` -- sorting shifted cores by raw local timestamps groups two wins that
/// are separated by a loss in true UTC and therefore inflates the global winning streak.
#[test]
fn shifted_two_core_rows_accumulate_in_true_utc_chronology() {
    let conn = Connection::open_in_memory().expect("in-memory database");
    conn.execute_batch(
        "CREATE TABLE current_rows(
            closedate INTEGER, buydate INTEGER, pnl REAL, core_uid INTEGER,
            core_name TEXT, coin TEXT, strategyid INTEGER, isshort INTEGER,
            profitbtc REAL, spentbtc REAL, basecurrency INTEGER
         );
         INSERT INTO current_rows VALUES
            (103600, 103540, 5.0, 1, 'ahead', 'BTC', 7, 0, 5.0, 20.0, 1),
            (96500, 96440, -2.0, 2, 'behind', 'ETH', 8, 0, -2.0, 20.0, 1),
            (103800, 103740, 4.0, 1, 'ahead', 'SOL', 7, 0, 4.0, 20.0, 1);",
    )
    .expect("shifted chronology fixture");
    let axis = crate::db::ReportAxis::from_measured(
        HashMap::from([
            (
                1,
                vec![crate::db::OffsetSegment {
                    from_utc: 0,
                    offset_secs: 3_600,
                }],
            ),
            (
                2,
                vec![crate::db::OffsetSegment {
                    from_utc: 0,
                    offset_secs: -3_600,
                }],
            ),
        ]),
        chrono_tz::UTC,
    );
    let query = Query {
        from: 99_900,
        to: 100_300,
        ..Default::default()
    };
    let source = "(SELECT * FROM current_rows WHERE ?1 <= ?2) o";

    let result = read(&conn, source, None, &query, &axis, 3_600, false, false)
        .expect("shifted chronological stream");

    assert_eq!(result.cur.n, 3);
    assert_eq!(result.cur.win_streak, 1);
    assert_eq!(result.cur.loss_streak, 1);
    assert_eq!(result.cur.max_dd, 2.0);
}

/// `inner_window_bounds` -- the earliest row under UTC-12 must survive a source predicate whose
/// whole-period offset was baked at UTC+14, the maximum valid retained-offset difference.
#[test]
fn shifted_window_padding_keeps_extreme_historical_boundary_row() {
    let conn = Connection::open_in_memory().expect("in-memory database");
    let from = 200_000;
    let transition = from + QUERY_WINDOW_SECS;
    let stored_at_lower_bound = from + i64::from(crate::db::report_axis::MIN_OFFSET_SECS);
    conn.execute_batch(&format!(
        "CREATE TABLE current_rows(
            closedate INTEGER, buydate INTEGER, pnl REAL, core_uid INTEGER,
            core_name TEXT, coin TEXT, strategyid INTEGER, isshort INTEGER,
            profitbtc REAL, spentbtc REAL, basecurrency INTEGER
         );
         INSERT INTO current_rows VALUES
            ({stored_at_lower_bound}, {stored_at_lower_bound}, 3.0, 1, 'shifted', 'EDGE',
             7, 0, 3.0, 10.0, 1);"
    ))
    .expect("extreme shifted boundary fixture");
    let axis = crate::db::ReportAxis::from_measured(
        HashMap::from([(
            1,
            vec![
                crate::db::OffsetSegment {
                    from_utc: 0,
                    offset_secs: crate::db::report_axis::MIN_OFFSET_SECS,
                },
                crate::db::OffsetSegment {
                    from_utc: transition,
                    offset_secs: crate::db::report_axis::MAX_OFFSET_SECS,
                },
            ],
        )]),
        chrono_tz::UTC,
    );
    let query = Query {
        from,
        to: from + 2 * QUERY_WINDOW_SECS,
        ..Default::default()
    };
    let baked = crate::db::report_axis::MAX_OFFSET_SECS;
    let source = format!(
        "(SELECT * FROM current_rows
          WHERE closedate >= ?1 + {baked} AND closedate < ?2 + {baked}) o"
    );

    let result = read(&conn, &source, None, &query, &axis, 86_400, false, false)
        .expect("extreme shifted boundary remains in the exact outer window");

    assert_eq!(result.cur.n, 1);
    assert_eq!(result.cur.profit, 3.0);
    assert_eq!(result.best[0].closedate, from);
}

/// `current_stream_sql` -- each production-shaped UNION branch must retain an indexed bounded
/// `closedate` search even when the exact shifted residual requires a scalar and a temp sort.
#[test]
fn indexed_union_plan_searches_each_source_inside_bounded_window() {
    let conn = Connection::open_in_memory().expect("in-memory database");
    conn.execute_batch(
        "CREATE TABLE current_a(
            closedate INTEGER, buydate INTEGER, pnl REAL, core_uid INTEGER,
            core_name TEXT, coin TEXT, strategyid INTEGER, isshort INTEGER,
            profitbtc REAL, spentbtc REAL, basecurrency INTEGER
         );
         CREATE TABLE current_b(
            closedate INTEGER, buydate INTEGER, pnl REAL, core_uid INTEGER,
            core_name TEXT, coin TEXT, strategyid INTEGER, isshort INTEGER,
            profitbtc REAL, spentbtc REAL, basecurrency INTEGER
         );
         CREATE INDEX current_a_closedate ON current_a(closedate);
         CREATE INDEX current_b_closedate ON current_b(closedate);",
    )
    .expect("indexed UNION plan fixture");
    let axis = crate::db::ReportAxis::from_measured(
        HashMap::from([(
            1,
            vec![crate::db::OffsetSegment {
                from_utc: 0,
                offset_secs: 3_600,
            }],
        )]),
        chrono_tz::UTC,
    );
    super::super::time_zone::install(&conn, &axis).expect("shifted scalar");
    let source = "(SELECT * FROM current_a WHERE closedate >= ?1 AND closedate < ?2
                   UNION ALL
                   SELECT * FROM current_b WHERE closedate >= ?1 AND closedate < ?2) o";
    let sql = current_stream_sql(
        source,
        "mt_to_utc(o.closedate, o.core_uid)",
        "o.profitbtc, o.spentbtc, o.basecurrency",
    );
    let (inner_from, inner_to) = inner_window_bounds(&axis, 100_000, 100_000 + QUERY_WINDOW_SECS);
    assert_eq!(inner_to - inner_from, MAX_INNER_SCAN_SECS);
    let mut statement = conn
        .prepare(&format!("EXPLAIN QUERY PLAN {sql}"))
        .expect("prepare current-stream plan");
    let details = statement
        .query_map(
            rusqlite::params![inner_from, inner_to, 100_000, 100_000 + QUERY_WINDOW_SECS],
            |row| row.get::<_, String>(3),
        )
        .expect("read current-stream plan")
        .collect::<rusqlite::Result<Vec<_>>>()
        .expect("collect current-stream plan");

    for table in ["current_a", "current_b"] {
        assert!(
            details.iter().any(|detail| {
                detail.contains(&format!("SEARCH {table}"))
                    && detail.contains("closedate>?")
                    && detail.contains("closedate<?")
            }),
            "{table} must use a bounded index search: {details:#?}"
        );
        assert!(
            !details
                .iter()
                .any(|detail| detail.contains(&format!("SCAN {table}"))),
            "{table} must not scan all history: {details:#?}"
        );
    }
}

/// `current_stream_sql` -- reversing indexed UNION branches at equal true-UTC times must leave
/// sequence metrics and the five-row equal-profit cutoff unchanged.
#[test]
fn equal_time_mixed_pnl_and_top_rows_ignore_union_planner_order() {
    let conn = Connection::open_in_memory().expect("in-memory database");
    conn.execute_batch(
        "CREATE TABLE tie_a(
            closedate INTEGER, buydate INTEGER, pnl REAL, core_uid INTEGER,
            core_name TEXT, coin TEXT, strategyid INTEGER, isshort INTEGER,
            profitbtc REAL, spentbtc REAL, basecurrency INTEGER
         );
         CREATE TABLE tie_b(
            closedate INTEGER, buydate INTEGER, pnl REAL, core_uid INTEGER,
            core_name TEXT, coin TEXT, strategyid INTEGER, isshort INTEGER,
            profitbtc REAL, spentbtc REAL, basecurrency INTEGER
         );
         CREATE INDEX tie_a_closedate ON tie_a(closedate);
         CREATE INDEX tie_b_closedate ON tie_b(closedate);
         INSERT INTO tie_a VALUES
            (100, 90,  5.0, 2, 'two', 'A2', 7, 0,  5.0, 10.0, 1),
            (100, 90, -4.0, 2, 'two', 'Z2', 7, 1, -4.0, 10.0, 1),
            (100, 90,  5.0, 2, 'two', 'B2', 7, 0,  5.0, 10.0, 1),
            (100, 90,  5.0, 2, 'two', 'C2', 7, 0,  5.0, 10.0, 1);
         INSERT INTO tie_b VALUES
            (100, 90,  5.0, 1, 'one', 'A1', 7, 0,  5.0, 10.0, 1),
            (100, 90, -3.0, 1, 'one', 'Z1', 7, 1, -3.0, 10.0, 1),
            (100, 90,  5.0, 1, 'one', 'B1', 7, 0,  5.0, 10.0, 1),
            (100, 90,  5.0, 1, 'one', 'C1', 7, 0,  5.0, 10.0, 1);",
    )
    .expect("equal-time UNION fixture");
    let query = Query {
        from: 1,
        to: 300,
        ..Default::default()
    };
    let forward = "(SELECT * FROM tie_a WHERE closedate >= ?1 AND closedate < ?2
                     UNION ALL
                     SELECT * FROM tie_b WHERE closedate >= ?1 AND closedate < ?2) o";
    let reverse = "(SELECT * FROM tie_b WHERE closedate >= ?1 AND closedate < ?2
                     UNION ALL
                     SELECT * FROM tie_a WHERE closedate >= ?1 AND closedate < ?2) o";
    let left = read(
        &conn,
        forward,
        None,
        &query,
        &query.axis,
        3_600,
        false,
        false,
    )
    .expect("forward UNION result");
    let right = read(
        &conn,
        reverse,
        None,
        &query,
        &query.axis,
        3_600,
        false,
        false,
    )
    .expect("reverse UNION result");
    let top = |rows: &[TopTrade]| {
        rows.iter()
            .map(|row| {
                (
                    row.coin.clone(),
                    row.strategy.clone(),
                    row.core_name.clone(),
                    row.profit.to_bits(),
                    row.is_short,
                )
            })
            .collect::<Vec<_>>()
    };

    assert_eq!(left.cur.max_dd, 4.0);
    assert_eq!(left.cur.win_streak, 3);
    assert_eq!(left.cur.loss_streak, 1);
    assert_eq!(left.cur.max_dd, right.cur.max_dd);
    assert_eq!(left.cur.win_streak, right.cur.win_streak);
    assert_eq!(left.cur.loss_streak, right.cur.loss_streak);
    assert_eq!(top(&left.best), top(&right.best));
    assert_eq!(top(&left.worst), top(&right.worst));
    assert_eq!(
        left.best
            .iter()
            .map(|row| row.coin.as_str())
            .collect::<Vec<_>>(),
        vec!["A1", "B1", "C1", "A2", "B2"]
    );
}

/// `summary_stream::read` -- routing a zero-offset axis through `mt_to_utc` restores the costly
/// scalar sort, while routing a shifted axis by raw `closedate` changes cross-core chronology.
#[test]
fn current_summary_ordering_uses_raw_dates_only_for_zero_offset_axes() {
    let _trace = start_trace();
    let conn = Connection::open_in_memory().expect("in-memory database");
    conn.execute_batch(
        "CREATE TABLE current_rows(
            closedate INTEGER, buydate INTEGER, pnl REAL, core_uid INTEGER,
            core_name TEXT, coin TEXT, strategyid INTEGER, isshort INTEGER,
            profitbtc REAL, spentbtc REAL, basecurrency INTEGER
         );
         INSERT INTO current_rows VALUES
            (100, 90, 2.0, 1, 'alpha', 'BTC', 7, 0, 2.0, 20.0, 1);",
    )
    .expect("summary ordering fixture");
    let query = Query {
        from: 1,
        to: 300,
        ..Default::default()
    };
    let source = "(SELECT * FROM current_rows WHERE closedate >= ?1 AND closedate < ?2) o";
    let zero_axis = crate::db::ReportAxis::from_measured(
        HashMap::from([(
            1,
            vec![crate::db::OffsetSegment {
                from_utc: 0,
                offset_secs: 0,
            }],
        )]),
        chrono_tz::UTC,
    );

    conn.trace_v2(TraceEventCodes::SQLITE_TRACE_STMT, Some(record_sql));
    read(&conn, source, None, &query, &zero_axis, 3_600, true, false)
        .expect("zero-offset current-period stream");
    conn.trace_v2(TraceEventCodes::empty(), None);
    let zero_sql = TRACE_SQL
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone();
    assert_eq!(zero_sql.len(), 1);
    assert!(zero_sql[0].contains("ORDER BY o.closedate"));
    assert!(!zero_sql[0].contains("mt_to_utc"));

    TRACE_SQL
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clear();
    let shifted_axis = crate::db::ReportAxis::from_measured(
        HashMap::from([(
            1,
            vec![crate::db::OffsetSegment {
                from_utc: 0,
                offset_secs: 3_600,
            }],
        )]),
        chrono_tz::UTC,
    );
    conn.trace_v2(TraceEventCodes::SQLITE_TRACE_STMT, Some(record_sql));
    read(
        &conn,
        source,
        None,
        &query,
        &shifted_axis,
        3_600,
        true,
        false,
    )
    .expect("shifted current-period stream");
    conn.trace_v2(TraceEventCodes::empty(), None);
    let shifted_sql = TRACE_SQL
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone();
    assert_eq!(shifted_sql.len(), 1);
    assert!(shifted_sql[0].contains("ORDER BY mt_to_utc(o.closedate, o.core_uid)"));
}

/// A non-integer SQLite storage class keeps the historical string strategy fallback.
///
/// Decoding `summary_stream.rs:TradeRow::strategy_id` directly as `Option<i64>` makes one legacy
/// text value fail the entire Summary instead of producing the key and top-row label used before.
#[test]
fn textual_strategy_storage_keeps_group_and_top_fallbacks() {
    let conn = Connection::open_in_memory().expect("in-memory database");
    conn.execute_batch(
        "CREATE TABLE current_rows(
            closedate INTEGER, buydate INTEGER, pnl REAL, core_uid INTEGER,
            core_name TEXT, coin TEXT, strategyid INTEGER, isshort INTEGER,
            profitbtc REAL, spentbtc REAL, basecurrency INTEGER
         );
         INSERT INTO current_rows VALUES
            (100, 90, 2.0, 1, 'alpha', 'BTC', 'odd-id', 0, 2.0, 20.0, 1);",
    )
    .expect("textual strategy fixture");
    let query = Query {
        from: 1,
        to: 300,
        ..Default::default()
    };
    let source = "(SELECT * FROM current_rows WHERE closedate >= ?1 AND closedate < ?2) o";

    let result = read(&conn, source, None, &query, &query.axis, 3_600, true, false)
        .expect("textual strategy remains readable");

    assert_eq!(result.strategies.len(), 1);
    assert_eq!(result.strategies[0].key, "odd-id@1");
    assert_eq!(result.strategies[0].name, "odd-id");
    assert_eq!(result.best[0].strategy, "odd-id");
    assert_eq!(result.worst[0].strategy, "odd-id");
}
